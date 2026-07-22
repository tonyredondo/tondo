use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::package::{ModuleId, SymbolIdentity};
use crate::resolve::{LocalKind, MemberKind, MemberOwner, ResolvedProgram, SymbolKind};
use crate::types::{
    CursorMode, FunctionParameter, FunctionType, GeneratedTypeKind, IntrinsicType, ParameterMode,
    ScalarType, TypeId, TypeInterner, TypeKind, TypeSubstitution,
};

use super::capabilities::{CapabilityAnalysis, CapabilityAssumptions, bounds_imply};
use super::termination::{TraitTerminationEdge, analyze_trait_termination};
use super::{
    AvailabilityFindingKind, HirAssignmentTarget, HirAssignmentTargetKind, HirBinaryOperator,
    HirCallableId, HirCapability, HirCapabilityStatus, HirClosureId, HirConstantValue,
    HirConstantValueKind, HirConstantVariantValue, HirContainmentKind, HirExpression,
    HirExpressionId, HirExpressionKind, HirFlow, HirForKind, HirGenericParameter, HirIndexAccess,
    HirIterationProtocol, HirPattern, HirPatternId, HirPatternKind, HirProgram, HirStatement,
    HirTraitConstructor, HirTraitIdentity, HirTraitMethodKey, HirTypeDeclarationKind,
    HirValueCategory, HirVariantPayload, HirVariantValue, TraitQuery, TraitSelectionError,
    analyze_availability, select_implementation,
};

/// Reports a compiler defect at the boundary between typed HIR and MIR.
///
/// This is deliberately not a Tondo source diagnostic: accepted source can
/// never cause a failed invariant without a bug in an earlier compiler phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirInvariantError {
    context: String,
    message: String,
}

impl HirInvariantError {
    fn new(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            message: message.into(),
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for HirInvariantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "typed HIR invariant failed for {}: {}",
            self.context, self.message
        )
    }
}

impl Error for HirInvariantError {}

/// Verifies the closed typed-HIR contract required by MIR lowering.
///
/// Partial semantic snapshots intentionally do not satisfy this contract. The
/// driver and tooling may retain those snapshots, but MIR must only consume a
/// program accepted by this verifier.
pub(crate) fn verify_typed_hir(
    resolved: &ResolvedProgram,
    program: &HirProgram,
) -> Result<(), HirInvariantError> {
    Verifier { resolved, program }.verify()
}

struct Verifier<'a> {
    resolved: &'a ResolvedProgram,
    program: &'a HirProgram,
}

struct CallProtocolVerification<'a> {
    assumptions: &'a BTreeSet<TraitQuery>,
    capabilities: &'a CapabilityAssumptions,
    analysis: &'a CapabilityAnalysis,
    exclusive_parameters: &'a BTreeSet<crate::resolve::LocalId>,
    mutable_receiver: bool,
    context: &'a str,
}

struct OpaqueTraitProof {
    interner: TypeInterner,
    assumptions: BTreeSet<TraitQuery>,
    capability_analysis: CapabilityAnalysis,
    capability_assumptions: CapabilityAssumptions,
    active: BTreeSet<TraitQuery>,
    memo: BTreeMap<TraitQuery, bool>,
    context: String,
}

impl Verifier<'_> {
    fn verify(&self) -> Result<(), HirInvariantError> {
        if !self.program.expression_check_complete {
            return Err(HirInvariantError::new(
                "program",
                "expression checking is incomplete",
            ));
        }
        if self.program.expression_flows.len() != self.program.expressions.len()
            || self.program.expression_breaks.len() != self.program.expressions.len()
        {
            return Err(HirInvariantError::new(
                "expression arena",
                format!(
                    "{} expressions, {} flow summaries, and {} break summaries are not aligned",
                    self.program.expressions.len(),
                    self.program.expression_flows.len(),
                    self.program.expression_breaks.len()
                ),
            ));
        }
        if self.program.capability_statuses.len() != self.program.interner.len() {
            return Err(HirInvariantError::new(
                "type capabilities",
                format!(
                    "{} types and {} capability rows are not aligned",
                    self.program.interner.len(),
                    self.program.capability_statuses.len()
                ),
            ));
        }
        self.verify_capability_statuses()?;

        self.verify_declarations()?;
        self.verify_implementations()?;
        self.verify_constants()?;
        self.verify_callables()?;
        self.verify_closures()?;
        self.verify_capability_contracts()?;
        self.verify_annotations_and_locals()?;
        self.verify_member_references()?;
        self.verify_patterns()?;
        let loops = self.collect_loops()?;
        self.verify_expressions(&loops)?;
        self.verify_call_protocol_contracts()?;
        self.verify_bodies()?;
        self.verify_availability()?;
        Ok(())
    }

    fn verify_availability(&self) -> Result<(), HirInvariantError> {
        let capabilities = CapabilityAnalysis::new(self.program, self.resolved)
            .map_err(|error| HirInvariantError::new("ownership availability", error.to_string()))?;
        if let Some(finding) = analyze_availability(self.program, &capabilities)
            .map_err(|error| HirInvariantError::new("ownership availability", error.to_string()))?
            .into_iter()
            .next()
        {
            let local = finding
                .local()
                .map(|local| format!("local#{}", local.index()))
                .unwrap_or_else(|| "receiver or temporary".into());
            let message = match finding.kind() {
                AvailabilityFindingKind::UseAfterMove => format!(
                    "{local} is used at {} after its value moved at {}",
                    finding.use_span().range(),
                    finding
                        .move_span()
                        .expect("use-after-move findings retain their origin")
                        .range()
                ),
                AvailabilityFindingKind::InvalidPartialTransfer => format!(
                    "{local} has a non-Copy partial transfer at {}",
                    finding.use_span().range()
                ),
                AvailabilityFindingKind::InvalidBorrowedTransfer => format!(
                    "{local} transfers ownership through a borrowed location at {}",
                    finding.use_span().range()
                ),
                AvailabilityFindingKind::InvalidGuardAccess => format!(
                    "{local} is accessed as an affine guard binding at {}",
                    finding.use_span().range()
                ),
                AvailabilityFindingKind::InvalidMatchMode => format!(
                    "match at {} has an ownership mode inconsistent with its scrutinee and bindings",
                    finding.use_span().range()
                ),
            };
            return Err(HirInvariantError::new("ownership availability", message));
        }
        Ok(())
    }

    fn verify_capability_statuses(&self) -> Result<(), HirInvariantError> {
        let analysis = CapabilityAnalysis::new(self.program, self.resolved)
            .map_err(|error| HirInvariantError::new("type capabilities", error.to_string()))?;
        let assumptions = CapabilityAssumptions::default();
        for ty in self.program.interner.ids() {
            for capability in HirCapability::ALL {
                let expected = analysis
                    .status(self.program, ty, capability, &assumptions)
                    .map_err(|error| {
                        HirInvariantError::new("type capabilities", error.to_string())
                    })?;
                let actual = self
                    .program
                    .capability_status(ty, capability)
                    .ok_or_else(|| {
                        HirInvariantError::new(
                            "type capabilities",
                            "capability table omitted an interned type",
                        )
                    })?;
                if actual != expected {
                    return Err(HirInvariantError::new(
                        "type capabilities",
                        format!(
                            "{} status for {} is {:?}, expected {:?}",
                            capability.as_str(),
                            ty,
                            actual,
                            expected
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn verify_capability_contracts(&self) -> Result<(), HirInvariantError> {
        let analysis = CapabilityAnalysis::new(self.program, self.resolved)
            .map_err(|error| HirInvariantError::new("capability contracts", error.to_string()))?;

        for declaration in self.program.declarations.values() {
            let context = format!("type declaration symbol#{}", declaration.symbol.index());
            let assumptions =
                CapabilityAssumptions::from_generics(self.program, &declaration.parameters);
            let mut roots = declaration
                .parameters
                .iter()
                .flat_map(generic_bound_type_roots)
                .collect::<Vec<_>>();
            match &declaration.kind {
                HirTypeDeclarationKind::Alias { target } => roots.push(*target),
                HirTypeDeclarationKind::Nominal(definition) => {
                    roots.extend(nominal_type_roots(&definition.shape));
                }
                HirTypeDeclarationKind::Trait(definition) => roots.push(definition.self_type),
            }
            self.verify_type_formations(&analysis, roots, &assumptions, &context)?;
        }

        for callable in &self.program.callables {
            let context = format!("callable {:?}", callable.id);
            let assumptions =
                CapabilityAssumptions::from_generics(self.program, &callable.generics);
            let mut roots = vec![callable.function_type];
            roots.extend(callable.generics.iter().flat_map(generic_bound_type_roots));
            self.verify_type_formations(&analysis, roots, &assumptions, &context)?;
        }

        for closure in &self.program.closures {
            let context = format!("closure#{}", closure.id.index());
            let assumptions = CapabilityAssumptions::from_generics(self.program, &closure.generics);
            let mut roots = vec![closure.ty, closure.function_type];
            roots.extend(closure.generics.iter().flat_map(generic_bound_type_roots));
            roots.extend(closure.captures.iter().map(|capture| capture.ty));
            self.verify_type_formations(&analysis, roots, &assumptions, &context)?;
            for capture in &closure.captures {
                self.verify_capability_requirement(
                    &analysis,
                    capture.ty,
                    HirCapability::Copy,
                    &assumptions,
                    &context,
                    "M4 closure capture",
                )?;
            }
        }

        let default_assumptions = CapabilityAssumptions::default();
        for constant in self.program.constants.values() {
            if let Some(ty) = constant.declared_type {
                self.verify_type_formations(
                    &analysis,
                    [ty],
                    &default_assumptions,
                    &format!("constant symbol#{}", constant.symbol.index()),
                )?;
            }
        }

        for implementation in &self.program.implementations {
            let context = format!("implementation#{}", implementation.id.index());
            let assumptions =
                CapabilityAssumptions::from_generics(self.program, &implementation.parameters);
            let mut roots = vec![implementation.target];
            roots.extend(implementation.trait_reference.arguments.iter().copied());
            roots.extend(
                implementation
                    .parameters
                    .iter()
                    .flat_map(generic_bound_type_roots),
            );
            self.verify_type_formations(&analysis, roots, &assumptions, &context)?;
            if implementation.requires_self_send {
                self.verify_capability_requirement(
                    &analysis,
                    implementation.target,
                    HirCapability::Send,
                    &assumptions,
                    &context,
                    "async trait receiver",
                )?;
            }
        }

        for (symbol, constant) in &self.program.constants {
            let Some(root) = constant.value else {
                continue;
            };
            self.verify_expression_capability_tree(
                root,
                &analysis,
                &default_assumptions,
                &format!("constant symbol#{}", symbol.index()),
            )?;
        }
        for (callable, body) in &self.program.bodies {
            let signature = self.program.callable(*callable).ok_or_else(|| {
                HirInvariantError::new(
                    "capability contracts",
                    "body has no callable capability context",
                )
            })?;
            let assumptions =
                CapabilityAssumptions::from_generics(self.program, &signature.generics);
            self.verify_expression_capability_tree(
                body.root,
                &analysis,
                &assumptions,
                &format!("callable {callable:?}"),
            )?;
        }
        for closure in &self.program.closures {
            let assumptions = CapabilityAssumptions::from_generics(self.program, &closure.generics);
            self.verify_expression_capability_tree(
                closure.body.root,
                &analysis,
                &assumptions,
                &format!("closure#{}", closure.id.index()),
            )?;
        }
        Ok(())
    }

    fn verify_type_formations(
        &self,
        analysis: &CapabilityAnalysis,
        roots: impl IntoIterator<Item = TypeId>,
        assumptions: &CapabilityAssumptions,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let mut pending = roots.into_iter().collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.program.interner.kind(ty).map_err(|error| {
                HirInvariantError::new(context, format!("invalid formation type: {error}"))
            })? {
                TypeKind::Function(function) => {
                    pending.extend(function.parameters().iter().map(|parameter| parameter.ty()));
                    pending.extend(function.variadic());
                    pending.push(function.outcome());
                }
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Generated { arguments, .. }
                | TypeKind::OpaqueResult { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => {
                    let requirement = match constructor {
                        IntrinsicType::Map => {
                            Some((arguments[0], HirCapability::Key, "Map key formation"))
                        }
                        IntrinsicType::Set => {
                            Some((arguments[0], HirCapability::Key, "Set key formation"))
                        }
                        IntrinsicType::Ref => {
                            Some((arguments[0], HirCapability::Discard, "Ref target formation"))
                        }
                        IntrinsicType::Array
                        | IntrinsicType::Range
                        | IntrinsicType::Pointer
                        | IntrinsicType::Join
                        | IntrinsicType::Command
                        | IntrinsicType::Pipeline
                        | IntrinsicType::NumericConversionError => None,
                    };
                    if let Some((required, capability, reason)) = requirement {
                        self.verify_capability_requirement(
                            analysis,
                            required,
                            capability,
                            assumptions,
                            context,
                            reason,
                        )?;
                    }
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_) => {}
            }
        }
        Ok(())
    }

    fn verify_capability_requirement(
        &self,
        analysis: &CapabilityAnalysis,
        ty: TypeId,
        capability: HirCapability,
        assumptions: &CapabilityAssumptions,
        context: &str,
        reason: &str,
    ) -> Result<(), HirInvariantError> {
        let status = analysis
            .status(self.program, ty, capability, assumptions)
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
        if status != HirCapabilityStatus::Satisfied {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "{} requires {} for type {}, found {status:?}",
                    reason,
                    capability.as_str(),
                    ty
                ),
            ));
        }
        Ok(())
    }

    fn verify_expression_capability_tree(
        &self,
        root: HirExpressionId,
        analysis: &CapabilityAnalysis,
        assumptions: &CapabilityAssumptions,
        owner: &str,
    ) -> Result<(), HirInvariantError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let expression = self.program.expression(id).ok_or_else(|| {
                HirInvariantError::new(owner, format!("unknown expression#{}", id.index()))
            })?;
            let context = format!("{owner} expression#{}", id.index());
            self.verify_expression_capability_contracts(
                expression,
                analysis,
                assumptions,
                &context,
            )?;
            pending.extend(expression_children(expression));
        }
        Ok(())
    }

    fn verify_expression_capability_contracts(
        &self,
        expression: &HirExpression,
        analysis: &CapabilityAnalysis,
        assumptions: &CapabilityAssumptions,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        self.verify_type_formations(analysis, [expression.ty], assumptions, context)?;
        match &expression.kind {
            HirExpressionKind::SpecializedFunction {
                callable,
                arguments,
            } => {
                self.verify_type_formations(
                    analysis,
                    arguments.iter().copied(),
                    assumptions,
                    context,
                )?;
                let signature = self.program.callable(*callable).ok_or_else(|| {
                    HirInvariantError::new(context, "specialization has no callable contract")
                })?;
                for parameter in &signature.generics {
                    let Some(argument) = arguments.get(parameter.position as usize).copied() else {
                        continue;
                    };
                    for bound in &parameter.bounds {
                        let HirTraitConstructor::Prelude(name) = &bound.constructor else {
                            continue;
                        };
                        if let Some(capability) = HirCapability::from_name(name.as_str()) {
                            self.verify_capability_requirement(
                                analysis,
                                argument,
                                capability,
                                assumptions,
                                context,
                                "generic specialization",
                            )?;
                        }
                    }
                }
            }
            HirExpressionKind::PreludeTraitFunction { arguments, .. } => {
                self.verify_type_formations(
                    analysis,
                    arguments.iter().copied(),
                    assumptions,
                    context,
                )?;
            }
            HirExpressionKind::Coerce {
                kind: crate::types::Assignability::CallableErasure,
                value,
            } => {
                let actual = self.expression(*value, context)?.ty;
                for capability in [
                    HirCapability::Copy,
                    HirCapability::Send,
                    HirCapability::Share,
                ] {
                    self.verify_capability_requirement(
                        analysis,
                        actual,
                        capability,
                        assumptions,
                        context,
                        "closure-to-function coercion",
                    )?;
                }
            }
            HirExpressionKind::Binary {
                operator: HirBinaryOperator::Equal | HirBinaryOperator::NotEqual,
                left,
                ..
            } => {
                let ty = self.expression(*left, context)?.ty;
                self.verify_capability_requirement(
                    analysis,
                    ty,
                    HirCapability::Equatable,
                    assumptions,
                    context,
                    "equality",
                )?;
            }
            HirExpressionKind::Contains {
                kind, container, ..
            } => {
                let container = self.expression(*container, context)?;
                let requirement = match (kind, self.program.interner.kind(container.ty)) {
                    (
                        HirContainmentKind::Array,
                        Ok(TypeKind::Intrinsic {
                            constructor: IntrinsicType::Array,
                            arguments,
                        }),
                    ) => Some((arguments[0], HirCapability::Equatable)),
                    (
                        HirContainmentKind::MapKey,
                        Ok(TypeKind::Intrinsic {
                            constructor: IntrinsicType::Map,
                            arguments,
                        }),
                    )
                    | (
                        HirContainmentKind::Set,
                        Ok(TypeKind::Intrinsic {
                            constructor: IntrinsicType::Set,
                            arguments,
                        }),
                    ) => Some((arguments[0], HirCapability::Key)),
                    (HirContainmentKind::Range | HirContainmentKind::StringChar, Ok(_)) => None,
                    _ => {
                        return Err(HirInvariantError::new(
                            context,
                            "membership kind does not match its container type",
                        ));
                    }
                };
                if let Some((ty, capability)) = requirement {
                    self.verify_capability_requirement(
                        analysis,
                        ty,
                        capability,
                        assumptions,
                        context,
                        "membership",
                    )?;
                }
            }
            HirExpressionKind::Index {
                base,
                access: HirIndexAccess::MapLookup,
                ..
            } => {
                let base = self.expression(*base, context)?;
                let Ok(TypeKind::Intrinsic {
                    constructor: IntrinsicType::Map,
                    arguments,
                }) = self.program.interner.kind(base.ty)
                else {
                    return Err(HirInvariantError::new(
                        context,
                        "map lookup has a non-map base",
                    ));
                };
                self.verify_capability_requirement(
                    analysis,
                    arguments[1],
                    HirCapability::Copy,
                    assumptions,
                    context,
                    "map lookup",
                )?;
            }
            HirExpressionKind::Block { statements, .. } => {
                for statement in statements {
                    self.verify_statement_capability_contracts(
                        statement,
                        analysis,
                        assumptions,
                        context,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn verify_statement_capability_contracts(
        &self,
        statement: &HirStatement,
        analysis: &CapabilityAnalysis,
        assumptions: &CapabilityAssumptions,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match statement {
            HirStatement::Binding {
                declared_type: Some(ty),
                ..
            } => self.verify_type_formations(analysis, [*ty], assumptions, context)?,
            HirStatement::Expression { value, .. } | HirStatement::Discard { value, .. } => {
                let ty = self.expression(*value, context)?.ty;
                self.verify_capability_requirement(
                    analysis,
                    ty,
                    HirCapability::Discard,
                    assumptions,
                    context,
                    "discarded statement value",
                )?;
            }
            HirStatement::Assignment { target, .. } => {
                let mut pending = vec![target];
                while let Some(target) = pending.pop() {
                    self.verify_type_formations(analysis, [target.ty], assumptions, context)?;
                    match &target.kind {
                        HirAssignmentTargetKind::Discard => {
                            self.verify_capability_requirement(
                                analysis,
                                target.ty,
                                HirCapability::Discard,
                                assumptions,
                                context,
                                "discard assignment target",
                            )?;
                        }
                        HirAssignmentTargetKind::Tuple(items) => pending.extend(items),
                        HirAssignmentTargetKind::Place { .. } => {}
                    }
                }
            }
            HirStatement::For {
                kind: HirForKind::Iterate { protocol, .. },
                ..
            } => match protocol {
                HirIterationProtocol::Intrinsic { cursor } => {
                    self.verify_type_formations(analysis, [*cursor], assumptions, context)?
                }
                HirIterationProtocol::Trait {
                    element,
                    function_type,
                } => self.verify_type_formations(
                    analysis,
                    [*element, *function_type],
                    assumptions,
                    context,
                )?,
            },
            HirStatement::Binding {
                declared_type: None,
                ..
            }
            | HirStatement::For { .. } => {}
        }
        Ok(())
    }

    fn verify_declarations(&self) -> Result<(), HirInvariantError> {
        for (key, declaration) in &self.program.declarations {
            let context = format!("type declaration symbol#{}", key.index());
            if *key != declaration.symbol {
                return Err(HirInvariantError::new(
                    context,
                    format!(
                        "arena key symbol#{} differs from the stored symbol#{}",
                        key.index(),
                        declaration.symbol.index()
                    ),
                ));
            }
            let expected = match declaration.kind {
                HirTypeDeclarationKind::Alias { .. } => &[SymbolKind::Alias][..],
                HirTypeDeclarationKind::Nominal(ref nominal) => match nominal.shape {
                    super::HirNominalShape::Enum { .. } => &[SymbolKind::Enum][..],
                    super::HirNominalShape::Newtype { .. }
                    | super::HirNominalShape::Record { .. } => &[SymbolKind::Type][..],
                },
                HirTypeDeclarationKind::Trait(_) => &[SymbolKind::Trait][..],
            };
            self.verify_symbol(declaration.symbol, expected, &context)?;
            self.verify_generics(
                &declaration.parameters,
                u32::try_from(declaration.parameters.len())
                    .map_err(|_| HirInvariantError::new(&context, "generic arity exceeds u32"))?,
                None,
                &context,
            )?;
            match &declaration.kind {
                HirTypeDeclarationKind::Alias { target } => {
                    self.verify_type(*target, format!("{context} alias target"))?;
                }
                HirTypeDeclarationKind::Nominal(nominal) => {
                    self.verify_type(nominal.self_type, format!("{context} self type"))?;
                    match &nominal.shape {
                        super::HirNominalShape::Newtype { underlying } => {
                            self.verify_type(*underlying, format!("{context} underlying type"))?;
                        }
                        super::HirNominalShape::Record { fields } => {
                            for field in fields {
                                self.verify_member(
                                    field.member,
                                    &[MemberKind::RecordField],
                                    &context,
                                )?;
                                self.verify_type(
                                    field.ty,
                                    format!("{context} field member#{}", field.member.index()),
                                )?;
                            }
                        }
                        super::HirNominalShape::Enum { variants } => {
                            for variant in variants {
                                self.verify_member(
                                    variant.member,
                                    &[MemberKind::EnumVariant],
                                    &context,
                                )?;
                                self.verify_variant_payload(
                                    &variant.payload,
                                    &format!("{context} variant member#{}", variant.member.index()),
                                )?;
                            }
                        }
                    }
                }
                HirTypeDeclarationKind::Trait(trait_definition) => {
                    self.verify_type(
                        trait_definition.self_type,
                        format!("{context} contextual Self"),
                    )?;
                    let expected_self =
                        u32::try_from(declaration.parameters.len()).map_err(|_| {
                            HirInvariantError::new(&context, "trait generic arity exceeds u32")
                        })?;
                    if !matches!(
                        self.program.interner.kind(trait_definition.self_type),
                        Ok(TypeKind::GenericParameter(position)) if *position == expected_self
                    ) {
                        return Err(HirInvariantError::new(
                            &context,
                            "trait contextual Self does not follow its declared parameters",
                        ));
                    }
                    let mut previous = None;
                    let mut actual_methods = BTreeSet::new();
                    for method in &trait_definition.methods {
                        if previous.is_some_and(|previous| previous >= method.member) {
                            return Err(HirInvariantError::new(
                                &context,
                                "trait methods are not in strict member-ID order",
                            ));
                        }
                        previous = Some(method.member);
                        actual_methods.insert(method.member);
                        let member = self.verify_member(
                            method.member,
                            &[MemberKind::TraitMethod, MemberKind::TraitAssociatedFunction],
                            &context,
                        )?;
                        if member.owner() != MemberOwner::Type(declaration.symbol) {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} belongs to another owner",
                                    method.member.index()
                                ),
                            ));
                        }
                        let callable = self
                            .program
                            .callable(HirCallableId::Member(method.member))
                            .ok_or_else(|| {
                                HirInvariantError::new(
                                    &context,
                                    format!(
                                        "trait method member#{} has no callable signature",
                                        method.member.index()
                                    ),
                                )
                            })?;
                        let expected_arity = expected_self
                            .checked_add(1)
                            .and_then(|arity| arity.checked_add(member.generic_arity()))
                            .ok_or_else(|| {
                                HirInvariantError::new(
                                    &context,
                                    "trait method generic arity exceeds u32",
                                )
                            })?;
                        if callable.generic_arity != expected_arity {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} has generic arity {}, expected {expected_arity}",
                                    method.member.index(),
                                    callable.generic_arity
                                ),
                            ));
                        }
                        if callable.generics.len() < declaration.parameters.len()
                            || !callable.generics.iter().zip(&declaration.parameters).all(
                                |(method, declaration)| same_generic_parameter(method, declaration),
                            )
                        {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} does not preserve the trait generic prefix",
                                    method.member.index()
                                ),
                            ));
                        }
                        if method.has_default != callable.body_source.is_some() {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} default-body flag is inconsistent",
                                    method.member.index()
                                ),
                            ));
                        }
                        let function = match self.program.interner.kind(callable.function_type) {
                            Ok(TypeKind::Function(function)) => function,
                            _ => continue,
                        };
                        let has_receiver = callable
                            .parameters
                            .iter()
                            .any(|parameter| parameter.receiver);
                        if matches!(member.kind(), MemberKind::TraitMethod) != has_receiver {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} has a receiver classification mismatch",
                                    method.member.index()
                                ),
                            ));
                        }
                        let requires_self_send = function.is_async() && has_receiver;
                        if method.requires_self_send != requires_self_send {
                            return Err(HirInvariantError::new(
                                &context,
                                format!(
                                    "trait method member#{} has an inconsistent Self: Send requirement",
                                    method.member.index()
                                ),
                            ));
                        }
                    }
                    let expected_methods = self
                        .resolved
                        .members()
                        .filter(|member| {
                            member.owner() == MemberOwner::Type(declaration.symbol)
                                && matches!(
                                    member.kind(),
                                    MemberKind::TraitMethod | MemberKind::TraitAssociatedFunction
                                )
                        })
                        .map(crate::resolve::Member::id)
                        .collect::<BTreeSet<_>>();
                    if actual_methods != expected_methods {
                        return Err(HirInvariantError::new(
                            &context,
                            "trait method table does not match the resolved declaration",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_variant_payload(
        &self,
        payload: &HirVariantPayload,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match payload {
            HirVariantPayload::Unit => {}
            HirVariantPayload::Tuple(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.verify_type(*item, format!("{context} tuple item {index}"))?;
                }
            }
            HirVariantPayload::Record(fields) => {
                for field in fields {
                    self.verify_member(field.member, &[MemberKind::VariantField], context)?;
                    self.verify_type(
                        field.ty,
                        format!("{context} field member#{}", field.member.index()),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn verify_implementations(&self) -> Result<(), HirInvariantError> {
        let mut table_callables = BTreeSet::new();
        let mut contract_interner = self.program.interner.clone();
        for (index, implementation) in self.program.implementations.iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| {
                HirInvariantError::new("implementation table", "implementation ID exceeds u32")
            })?;
            let context = format!("implementation#{index}");
            if implementation.id.index() != index {
                return Err(HirInvariantError::new(
                    &context,
                    format!(
                        "table position {index} contains implementation#{}",
                        implementation.id.index()
                    ),
                ));
            }
            if !implementation.contract_complete {
                return Err(HirInvariantError::new(
                    &context,
                    "implementation contract is incomplete",
                ));
            }
            let expected_module = self
                .resolved
                .modules()
                .find(|module| module.files().contains(&implementation.span.file()))
                .map(crate::resolve::ResolvedModule::id)
                .ok_or_else(|| {
                    HirInvariantError::new(&context, "implementation file has no resolved module")
                })?;
            if expected_module != &implementation.module {
                return Err(HirInvariantError::new(
                    &context,
                    format!(
                        "stored module {} differs from resolved module {expected_module}",
                        implementation.module
                    ),
                ));
            }
            let outer_arity = u32::try_from(implementation.parameters.len()).map_err(|_| {
                HirInvariantError::new(&context, "implementation generic arity exceeds u32")
            })?;
            self.verify_generics(&implementation.parameters, outer_arity, None, &context)?;
            self.verify_type(implementation.target, format!("{context} target"))?;
            for argument in &implementation.trait_reference.arguments {
                self.verify_type(*argument, format!("{context} trait argument"))?;
            }

            let source_trait = match &implementation.trait_reference.constructor {
                HirTraitConstructor::Symbol(symbol) => {
                    self.verify_symbol(*symbol, &[SymbolKind::Trait], &context)?;
                    let declaration = self.program.declaration(*symbol).ok_or_else(|| {
                        HirInvariantError::new(&context, "source trait has no HIR declaration")
                    })?;
                    if implementation.trait_reference.arguments.len()
                        != declaration.parameters.len()
                    {
                        return Err(HirInvariantError::new(
                            &context,
                            "trait argument arity does not match its declaration",
                        ));
                    }
                    Some(*symbol)
                }
                HirTraitConstructor::Prelude(name) => {
                    let expected = match name.as_str() {
                        "Display" => 0,
                        "Iterator" => 1,
                        _ => {
                            return Err(HirInvariantError::new(
                                &context,
                                format!("closed or unknown prelude trait `{name}` was admitted"),
                            ));
                        }
                    };
                    if implementation.trait_reference.arguments.len() != expected {
                        return Err(HirInvariantError::new(
                            &context,
                            "prelude trait argument arity is inconsistent",
                        ));
                    }
                    None
                }
                HirTraitConstructor::External(_) => {
                    return Err(HirInvariantError::new(
                        &context,
                        "external trait contract was admitted without an interface",
                    ));
                }
            };

            let mut header_positions = BTreeSet::new();
            self.collect_generic_positions(implementation.target, &mut header_positions)?;
            for argument in &implementation.trait_reference.arguments {
                self.collect_generic_positions(*argument, &mut header_positions)?;
            }
            if let Some(parameter) = implementation
                .parameters
                .iter()
                .find(|parameter| !header_positions.contains(&parameter.position))
            {
                return Err(HirInvariantError::new(
                    &context,
                    format!(
                        "generic local#{} does not occur in the coherence header",
                        parameter.local.index()
                    ),
                ));
            }
            self.verify_orphan_rule(implementation, &context)?;

            let mut provided = BTreeSet::new();
            for (method_index, method) in implementation.methods.iter().enumerate() {
                let method_index = u32::try_from(method_index).map_err(|_| {
                    HirInvariantError::new(&context, "implementation method ID exceeds u32")
                })?;
                let method_context = format!("{context} method#{method_index}");
                if method.id.implementation() != implementation.id
                    || method.id.index() != method_index
                {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "method ID does not match its implementation table position",
                    ));
                }
                if !table_callables.insert(HirCallableId::Implementation(method.id)) {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "implementation callable ID is duplicated",
                    ));
                }
                let contract = method.contract.as_ref().ok_or_else(|| {
                    HirInvariantError::new(&method_context, "method has no matched trait contract")
                })?;
                if !provided.insert(contract.method) {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "trait method contract is implemented more than once",
                    ));
                }
                let callable = self
                    .program
                    .callable(HirCallableId::Implementation(method.id))
                    .ok_or_else(|| {
                        HirInvariantError::new(
                            &method_context,
                            "implementation method has no callable signature",
                        )
                    })?;
                if callable.body_source.is_none() {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "implementation method has no source body",
                    ));
                }
                if callable.generics.len() < implementation.parameters.len()
                    || !callable
                        .generics
                        .iter()
                        .zip(&implementation.parameters)
                        .all(|(actual, expected)| same_generic_parameter(actual, expected))
                {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "callable does not preserve the implementation generic prefix",
                    ));
                }
                if callable.function_type != contract.function_type {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "callable signature differs from its instantiated trait contract",
                    ));
                }
                self.verify_type(
                    contract.function_type,
                    format!("{method_context} contract signature"),
                )?;
                let has_receiver = callable
                    .parameters
                    .iter()
                    .any(|parameter| parameter.receiver);
                if has_receiver != contract.has_receiver {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "receiver classification differs from the trait contract",
                    ));
                }
                let actual_bounds = callable
                    .generics
                    .iter()
                    .filter(|parameter| parameter.position >= outer_arity)
                    .map(|parameter| parameter.bounds.clone())
                    .collect::<Vec<_>>();
                if !same_generic_bound_groups(&actual_bounds, &contract.generic_bounds) {
                    return Err(HirInvariantError::new(
                        &method_context,
                        "method generic bounds differ from the trait contract",
                    ));
                }
                self.verify_implementation_method_key(
                    implementation,
                    source_trait,
                    method,
                    contract,
                    &mut contract_interner,
                    &method_context,
                )?;
            }

            let expected_send = if let Some(symbol) = source_trait {
                let declaration = self.program.declaration(symbol).expect("checked above");
                let HirTypeDeclarationKind::Trait(definition) = &declaration.kind else {
                    unreachable!("source trait kind was checked")
                };
                for expected in &definition.methods {
                    let key = HirTraitMethodKey::Source(expected.member);
                    if !expected.has_default && !provided.contains(&key) {
                        return Err(HirInvariantError::new(
                            &context,
                            format!(
                                "required trait method member#{} is missing",
                                expected.member.index()
                            ),
                        ));
                    }
                }
                definition
                    .methods
                    .iter()
                    .any(|method| method.requires_self_send)
            } else {
                let required = match &implementation.trait_reference.constructor {
                    HirTraitConstructor::Prelude(name) if name.as_str() == "Display" => {
                        HirTraitMethodKey::Prelude(super::HirPreludeTraitMethod::Display)
                    }
                    HirTraitConstructor::Prelude(name) if name.as_str() == "Iterator" => {
                        HirTraitMethodKey::Prelude(super::HirPreludeTraitMethod::IteratorNext)
                    }
                    _ => unreachable!("only open prelude traits reach this branch"),
                };
                if !provided.contains(&required) {
                    return Err(HirInvariantError::new(
                        &context,
                        "required prelude trait method is missing",
                    ));
                }
                false
            };
            if implementation.requires_self_send != expected_send {
                return Err(HirInvariantError::new(
                    &context,
                    "implementation has an inconsistent Self: Send requirement",
                ));
            }
        }
        self.verify_implementation_coherence()?;
        self.verify_trait_termination()?;

        let callable_ids = self
            .program
            .callables
            .iter()
            .filter_map(|callable| match callable.id {
                HirCallableId::Implementation(_) => Some(callable.id),
                HirCallableId::Symbol(_) | HirCallableId::Member(_) => None,
            })
            .collect::<BTreeSet<_>>();
        if callable_ids != table_callables {
            return Err(HirInvariantError::new(
                "implementation table",
                "implementation methods and callable signatures are not in one-to-one correspondence",
            ));
        }
        Ok(())
    }

    fn verify_implementation_coherence(&self) -> Result<(), HirInvariantError> {
        let mut groups = BTreeMap::<HirTraitIdentity, Vec<&super::HirImplementation>>::new();
        for implementation in &self.program.implementations {
            let context = format!("implementation#{}", implementation.id.index());
            let identity =
                self.trait_identity(&implementation.trait_reference.constructor, &context)?;
            groups.entry(identity).or_default().push(implementation);
        }

        for (identity, implementations) in groups {
            for left_index in 0..implementations.len() {
                for right_index in left_index + 1..implementations.len() {
                    let earlier = implementations[left_index];
                    let later = implementations[right_index];
                    let context = format!("implementation#{}", later.id.index());
                    if matches!(
                        &identity,
                        HirTraitIdentity::Prelude(name) if name.as_str() == "Iterator"
                    ) {
                        let earlier_element = earlier
                            .trait_reference
                            .arguments
                            .first()
                            .copied()
                            .ok_or_else(|| {
                                HirInvariantError::new(
                                    &context,
                                    "Iterator implementation has no element argument",
                                )
                            })?;
                        let later_element = later
                            .trait_reference
                            .arguments
                            .first()
                            .copied()
                            .ok_or_else(|| {
                                HirInvariantError::new(
                                    &context,
                                    "Iterator implementation has no element argument",
                                )
                            })?;
                        match self
                            .program
                            .interner
                            .first_order_independent_equivalent_after_unifying(
                                &[earlier.target],
                                &[later.target],
                                earlier_element,
                                later_element,
                            )
                            .map_err(|error| HirInvariantError::new(&context, error.to_string()))?
                        {
                            None => {}
                            Some(true) => {
                                return Err(HirInvariantError::new(
                                    &context,
                                    format!(
                                        "coherence header overlaps implementation#{}",
                                        earlier.id.index()
                                    ),
                                ));
                            }
                            Some(false) => {
                                return Err(HirInvariantError::new(
                                    &context,
                                    format!(
                                        "Iterator target conflicts functionally with implementation#{}",
                                        earlier.id.index()
                                    ),
                                ));
                            }
                        }
                        continue;
                    }

                    let mut earlier_header = earlier.trait_reference.arguments.clone();
                    earlier_header.push(earlier.target);
                    let mut later_header = later.trait_reference.arguments.clone();
                    later_header.push(later.target);
                    if self
                        .program
                        .interner
                        .first_order_independent_unifiable(&earlier_header, &later_header)
                        .map_err(|error| HirInvariantError::new(&context, error.to_string()))?
                    {
                        return Err(HirInvariantError::new(
                            &context,
                            format!(
                                "coherence header overlaps implementation#{}",
                                earlier.id.index()
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_trait_termination(&self) -> Result<(), HirInvariantError> {
        let mut edges = Vec::new();
        for (implementation_index, implementation) in
            self.program.implementations.iter().enumerate()
        {
            if implementation.parameters.is_empty() {
                continue;
            }
            let context = format!("implementation#{}", implementation.id.index());
            let source =
                self.trait_identity(&implementation.trait_reference.constructor, &context)?;
            let mut source_query = implementation.trait_reference.arguments.clone();
            source_query.push(implementation.target);
            for parameter in &implementation.parameters {
                let target = self
                    .program
                    .local_types
                    .get(&parameter.local)
                    .copied()
                    .ok_or_else(|| {
                        HirInvariantError::new(
                            &context,
                            format!(
                                "generic local#{} has no canonical type",
                                parameter.local.index()
                            ),
                        )
                    })?;
                if !matches!(
                    self.program.interner.kind(target),
                    Ok(TypeKind::GenericParameter(position)) if *position == parameter.position
                ) {
                    return Err(HirInvariantError::new(
                        &context,
                        format!(
                            "generic local#{} does not retain position {}",
                            parameter.local.index(),
                            parameter.position
                        ),
                    ));
                }
                for bound in &parameter.bounds {
                    let destination = self.trait_identity(&bound.constructor, &context)?;
                    if destination.is_closed_prelude() {
                        continue;
                    }
                    let mut destination_query = bound.arguments.clone();
                    destination_query.push(target);
                    edges.push(TraitTerminationEdge {
                        source: source.clone(),
                        source_query: source_query.clone(),
                        destination,
                        destination_query,
                        origin: implementation_index,
                    });
                }
            }
        }

        let failures = analyze_trait_termination(&self.program.interner, &edges, u64::MAX)
            .map_err(|error| HirInvariantError::new("trait termination", error.to_string()))?;
        if let Some(failure) = failures.first() {
            let origin = failure.origins().last().copied().ok_or_else(|| {
                HirInvariantError::new("trait termination", "failure has no witness edge")
            })?;
            return Err(HirInvariantError::new(
                format!("implementation#{origin}"),
                format!(
                    "nonterminating trait cycle `{}` has matrix `{}`",
                    failure
                        .traits()
                        .iter()
                        .map(HirTraitIdentity::canonical_name)
                        .collect::<Vec<_>>()
                        .join(" -> "),
                    failure.matrix().render()
                ),
            ));
        }
        Ok(())
    }

    fn trait_identity(
        &self,
        constructor: &HirTraitConstructor,
        context: &str,
    ) -> Result<HirTraitIdentity, HirInvariantError> {
        match constructor {
            HirTraitConstructor::Symbol(symbol) => self
                .resolved
                .symbol(*symbol)
                .map(|declaration| HirTraitIdentity::Symbol(declaration.identity().clone()))
                .ok_or_else(|| {
                    HirInvariantError::new(context, "trait identity has no resolved declaration")
                }),
            HirTraitConstructor::Prelude(name) => Ok(HirTraitIdentity::Prelude(name.clone())),
            HirTraitConstructor::External(identity) => {
                Ok(HirTraitIdentity::Symbol(identity.clone()))
            }
        }
    }

    fn verify_implementation_method_key(
        &self,
        implementation: &super::HirImplementation,
        source_trait: Option<crate::resolve::SymbolId>,
        method: &super::HirImplementationMethod,
        contract: &super::HirImplementationMethodContract,
        contract_interner: &mut TypeInterner,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match (contract.method, source_trait) {
            (HirTraitMethodKey::Source(member), Some(owner)) => {
                let declaration = self.verify_member(
                    member,
                    &[MemberKind::TraitMethod, MemberKind::TraitAssociatedFunction],
                    context,
                )?;
                if declaration.owner() != MemberOwner::Type(owner)
                    || declaration.name().as_str() != method.name.as_str()
                {
                    return Err(HirInvariantError::new(
                        context,
                        "source method key does not belong to the implemented trait and name",
                    ));
                }
                let trait_declaration = self.program.declaration(owner).expect("checked above");
                let HirTypeDeclarationKind::Trait(definition) = &trait_declaration.kind else {
                    unreachable!("source trait kind was checked")
                };
                let expected = definition
                    .methods
                    .iter()
                    .find(|expected| expected.member == member)
                    .ok_or_else(|| {
                        HirInvariantError::new(context, "method key is absent from the trait table")
                    })?;
                if contract.has_default != expected.has_default
                    || contract.requires_self_send != expected.requires_self_send
                {
                    return Err(HirInvariantError::new(
                        context,
                        "default or Self: Send metadata differs from the trait method",
                    ));
                }
                let source_callable = self
                    .program
                    .callable(HirCallableId::Member(member))
                    .ok_or_else(|| {
                        HirInvariantError::new(context, "trait method has no callable signature")
                    })?;
                let fixed_arity = u32::try_from(trait_declaration.parameters.len())
                    .ok()
                    .and_then(|arity| arity.checked_add(1))
                    .ok_or_else(|| {
                        HirInvariantError::new(context, "trait fixed arity exceeds u32")
                    })?;
                let local_arity = u32::try_from(contract.generic_bounds.len()).map_err(|_| {
                    HirInvariantError::new(context, "method generic arity exceeds u32")
                })?;
                let expected_arity = fixed_arity.checked_add(local_arity).ok_or_else(|| {
                    HirInvariantError::new(context, "method generic arity overflows u32")
                })?;
                if source_callable.generic_arity != expected_arity {
                    return Err(HirInvariantError::new(
                        context,
                        "stored contract has the wrong method-generic arity",
                    ));
                }
                let outer_arity = u32::try_from(implementation.parameters.len()).map_err(|_| {
                    HirInvariantError::new(context, "implementation generic arity exceeds u32")
                })?;
                let mut arguments = implementation.trait_reference.arguments.clone();
                arguments.push(implementation.target);
                let local_end = outer_arity.checked_add(local_arity).ok_or_else(|| {
                    HirInvariantError::new(context, "implementation method arity overflows u32")
                })?;
                for position in outer_arity..local_end {
                    arguments.push(
                        contract_interner
                            .generic_parameter(position)
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))?,
                    );
                }
                let substitution = TypeSubstitution::new(arguments);
                let expected_function = substitution
                    .apply(contract_interner, source_callable.function_type)
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
                if expected_function != contract.function_type {
                    return Err(HirInvariantError::new(
                        context,
                        "instantiated signature was not derived from the source trait method",
                    ));
                }
                let expected_bounds = source_callable
                    .generics
                    .iter()
                    .filter(|parameter| parameter.position >= fixed_arity)
                    .map(|parameter| {
                        parameter
                            .bounds
                            .iter()
                            .map(|bound| {
                                Ok(super::HirTraitReference {
                                    constructor: bound.constructor.clone(),
                                    arguments: bound
                                        .arguments
                                        .iter()
                                        .map(|argument| {
                                            substitution
                                                .apply(contract_interner, *argument)
                                                .map_err(|error| {
                                                    HirInvariantError::new(
                                                        context,
                                                        error.to_string(),
                                                    )
                                                })
                                        })
                                        .collect::<Result<Vec<_>, HirInvariantError>>()?,
                                })
                            })
                            .collect::<Result<Vec<_>, HirInvariantError>>()
                    })
                    .collect::<Result<Vec<_>, HirInvariantError>>()?;
                if !same_generic_bound_groups(&expected_bounds, &contract.generic_bounds) {
                    return Err(HirInvariantError::new(
                        context,
                        "instantiated generic bounds were not derived from the source trait method",
                    ));
                }
                if source_callable
                    .parameters
                    .iter()
                    .any(|parameter| parameter.receiver)
                    != contract.has_receiver
                {
                    return Err(HirInvariantError::new(
                        context,
                        "receiver metadata was not derived from the source trait method",
                    ));
                }
            }
            (HirTraitMethodKey::Prelude(method_key), None) => {
                let (trait_name, method_name) = match method_key {
                    super::HirPreludeTraitMethod::Display => ("Display", "display"),
                    super::HirPreludeTraitMethod::IteratorNext => ("Iterator", "next"),
                };
                if !matches!(
                    &implementation.trait_reference.constructor,
                    HirTraitConstructor::Prelude(name) if name.as_str() == trait_name
                ) || method.name.as_str() != method_name
                    || contract.has_default
                    || contract.requires_self_send
                {
                    return Err(HirInvariantError::new(
                        context,
                        "prelude method metadata does not match its contract",
                    ));
                }
                let (mode, outcome) = match method_key {
                    super::HirPreludeTraitMethod::Display => (
                        ParameterMode::Ref,
                        contract_interner.scalar(ScalarType::String),
                    ),
                    super::HirPreludeTraitMethod::IteratorNext => (
                        ParameterMode::Mut,
                        contract_interner
                            .option(implementation.trait_reference.arguments[0])
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))?,
                    ),
                };
                let expected_function = contract_interner
                    .function(FunctionType::new(
                        false,
                        false,
                        vec![FunctionParameter::new(mode, implementation.target)],
                        None,
                        outcome,
                    ))
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
                if expected_function != contract.function_type
                    || !contract.has_receiver
                    || !contract.generic_bounds.is_empty()
                {
                    return Err(HirInvariantError::new(
                        context,
                        "prelude method signature was not derived from its closed contract",
                    ));
                }
            }
            (HirTraitMethodKey::Source(_), None) | (HirTraitMethodKey::Prelude(_), Some(_)) => {
                return Err(HirInvariantError::new(
                    context,
                    "method key kind does not match the implemented trait",
                ));
            }
        }
        Ok(())
    }

    fn collect_generic_positions(
        &self,
        root: TypeId,
        positions: &mut BTreeSet<u32>,
    ) -> Result<(), HirInvariantError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.program.interner.kind(ty).map_err(|error| {
                HirInvariantError::new("implementation header", error.to_string())
            })? {
                TypeKind::GenericParameter(position) => {
                    positions.insert(*position);
                }
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Intrinsic { arguments, .. }
                | TypeKind::Generated { arguments, .. }
                | TypeKind::OpaqueResult { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters().iter().map(|parameter| parameter.ty()));
                    pending.extend(function.variadic());
                    pending.push(function.outcome());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error | TypeKind::Scalar(_) | TypeKind::Inference(_) => {}
            }
        }
        Ok(())
    }

    fn verify_orphan_rule(
        &self,
        implementation: &super::HirImplementation,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let owns_trait = match &implementation.trait_reference.constructor {
            HirTraitConstructor::Symbol(symbol) => {
                self.resolved.symbol(*symbol).is_some_and(|symbol| {
                    identity_belongs_to(&implementation.module, symbol.identity())
                })
            }
            HirTraitConstructor::External(identity) => {
                identity_belongs_to(&implementation.module, identity)
            }
            HirTraitConstructor::Prelude(_) => false,
        };
        let owns_target = match self.program.interner.kind(implementation.target) {
            Ok(TypeKind::Nominal { identity, .. } | TypeKind::OpaqueResult { identity, .. }) => {
                identity_belongs_to(&implementation.module, identity)
            }
            _ => false,
        };
        if owns_trait || owns_target {
            Ok(())
        } else {
            Err(HirInvariantError::new(
                context,
                "orphan implementation was admitted",
            ))
        }
    }

    fn verify_constants(&self) -> Result<(), HirInvariantError> {
        for (key, constant) in &self.program.constants {
            let context = format!("constant symbol#{}", key.index());
            if *key != constant.symbol {
                return Err(HirInvariantError::new(
                    context,
                    format!(
                        "arena key symbol#{} differs from the stored symbol#{}",
                        key.index(),
                        constant.symbol.index()
                    ),
                ));
            }
            self.verify_symbol(constant.symbol, &[SymbolKind::Constant], &context)?;
            if let Some(declared) = constant.declared_type {
                self.verify_type(declared, format!("{context} declared type"))?;
            }
            let ty = constant.ty.ok_or_else(|| {
                HirInvariantError::new(&context, "constant has no checked initializer type")
            })?;
            self.verify_type(ty, format!("{context} initializer type"))?;
            let value = constant.value.ok_or_else(|| {
                HirInvariantError::new(&context, "constant has no checked initializer expression")
            })?;
            let expression = self.expression(value, &context)?;
            if expression.ty != ty {
                return Err(HirInvariantError::new(
                    context,
                    format!(
                        "initializer expression#{} has {}, expected {}",
                        value.index(),
                        expression.ty,
                        ty
                    ),
                ));
            }
            let evaluated = constant.evaluated.as_ref().ok_or_else(|| {
                HirInvariantError::new(&context, "constant has no normalized compile-time value")
            })?;
            if evaluated.ty != ty {
                return Err(HirInvariantError::new(
                    &context,
                    format!("normalized value has {}, expected {}", evaluated.ty, ty),
                ));
            }
            self.verify_constant_value(evaluated, &context)?;
        }
        Ok(())
    }

    fn verify_constant_value(
        &self,
        root: &HirConstantValue,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let mut pending = vec![root];
        while let Some(value) = pending.pop() {
            self.verify_type(value.ty, format!("{context} normalized value"))?;
            match &value.kind {
                HirConstantValueKind::Unit
                | HirConstantValueKind::Bool(_)
                | HirConstantValueKind::Integer(_)
                | HirConstantValueKind::Float(_)
                | HirConstantValueKind::Char(_)
                | HirConstantValueKind::String(_)
                | HirConstantValueKind::OptionNone => {}
                HirConstantValueKind::Function {
                    callable,
                    arguments,
                } => {
                    self.verify_callable_id(*callable, context)?;
                    for argument in arguments {
                        self.verify_type(*argument, format!("{context} function argument"))?;
                    }
                }
                HirConstantValueKind::Tuple(values)
                | HirConstantValueKind::Array(values)
                | HirConstantValueKind::Set(values) => pending.extend(values),
                HirConstantValueKind::Map(entries) => {
                    for (key, value) in entries {
                        pending.push(key);
                        pending.push(value);
                    }
                }
                HirConstantValueKind::Newtype { constructor, value } => {
                    self.verify_symbol(*constructor, &[SymbolKind::Type], context)?;
                    pending.push(value);
                }
                HirConstantValueKind::Record { owner, fields } => {
                    self.verify_symbol(*owner, &[SymbolKind::Type], context)?;
                    for field in fields {
                        self.verify_member(field.member, &[MemberKind::RecordField], context)?;
                        pending.push(&field.value);
                    }
                }
                HirConstantValueKind::Variant { variant, payload } => {
                    self.verify_member(*variant, &[MemberKind::EnumVariant], context)?;
                    match payload {
                        HirConstantVariantValue::Unit => {}
                        HirConstantVariantValue::Tuple(values) => pending.extend(values),
                        HirConstantVariantValue::Record(fields) => {
                            for field in fields {
                                self.verify_member(
                                    field.member,
                                    &[MemberKind::VariantField],
                                    context,
                                )?;
                                pending.push(&field.value);
                            }
                        }
                    }
                }
                HirConstantValueKind::OptionSome(value)
                | HirConstantValueKind::ResultOk(value)
                | HirConstantValueKind::ResultErr(value)
                | HirConstantValueKind::Converted(value) => pending.push(value),
                HirConstantValueKind::Range { start, end, .. } => {
                    pending.push(start);
                    pending.push(end);
                }
            }
        }
        Ok(())
    }

    fn verify_callables(&self) -> Result<(), HirInvariantError> {
        let mut ids = BTreeSet::new();
        let mut opaque_ids = BTreeSet::new();
        let mut previous = None;
        for callable in &self.program.callables {
            let context = callable_context(callable.id);
            if !ids.insert(callable.id) {
                return Err(HirInvariantError::new(context, "callable ID is duplicated"));
            }
            if previous.is_some_and(|previous| previous >= callable.id) {
                return Err(HirInvariantError::new(
                    context,
                    "callables are not in strict deterministic ID order",
                ));
            }
            previous = Some(callable.id);
            self.verify_resolved_callable_id(callable.id, &context)?;
            let hidden = self.trait_self_position(callable.id);
            self.verify_generics(&callable.generics, callable.generic_arity, hidden, &context)?;
            self.verify_type(callable.outcome, format!("{context} outcome"))?;
            self.verify_type(callable.function_type, format!("{context} function type"))?;
            let TypeKind::Function(function) =
                self.program
                    .interner
                    .kind(callable.function_type)
                    .map_err(|error| HirInvariantError::new(&context, error.to_string()))?
            else {
                return Err(HirInvariantError::new(
                    &context,
                    "callable signature is not a function type",
                ));
            };
            if function.is_async()
                && callable.parameters.iter().any(|parameter| {
                    matches!(parameter.mode, ParameterMode::Mut | ParameterMode::Var)
                })
            {
                return Err(HirInvariantError::new(
                    &context,
                    "async callable retains an exclusive parameter across suspension",
                ));
            }
            self.verify_opaque_result(callable, &mut opaque_ids, &context)?;
            for (index, parameter) in callable.parameters.iter().enumerate() {
                self.verify_type(parameter.ty, format!("{context} parameter {index}"))?;
                if let Some(element) = parameter.variadic_element {
                    self.verify_type(element, format!("{context} variadic element"))?;
                }
                if parameter.receiver && parameter.local.is_some() {
                    return Err(HirInvariantError::new(
                        &context,
                        format!("receiver parameter {index} also has a local ID"),
                    ));
                }
                if let Some(local) = parameter.local {
                    self.verify_local(local, &context)?;
                }
            }
            if callable.body_source.is_some() && !self.program.bodies.contains_key(&callable.id) {
                return Err(HirInvariantError::new(
                    context,
                    "source body has no checked HIR body",
                ));
            }
        }
        Ok(())
    }

    fn verify_closures(&self) -> Result<(), HirInvariantError> {
        let mut identities = BTreeSet::new();
        let mut constructions = BTreeMap::<HirClosureId, (usize, crate::source::Span)>::new();
        for expression in &self.program.expressions {
            let HirExpressionKind::Closure(id) = &expression.kind else {
                continue;
            };
            constructions
                .entry(*id)
                .and_modify(|(count, _)| *count += 1)
                .or_insert((1, expression.span));
        }
        for (index, closure) in self.program.closures.iter().enumerate() {
            let id =
                HirClosureId(u32::try_from(index).map_err(|_| {
                    HirInvariantError::new("closures", "closure index exceeds u32")
                })?);
            let context = format!("closure#{}", id.index());
            if closure.id != id {
                return Err(HirInvariantError::new(
                    context,
                    "closure IDs are not dense in deterministic registration order",
                ));
            }
            if !identities.insert(closure.identity.clone()) {
                return Err(HirInvariantError::new(
                    context,
                    "generated closure identity is duplicated",
                ));
            }
            if closure.identity.start_byte() != closure.span.range().start() {
                return Err(HirInvariantError::new(
                    &context,
                    "generated closure identity has the wrong source position",
                ));
            }
            let Some((construction_count, construction_span)) = constructions.get(&id) else {
                return Err(HirInvariantError::new(
                    &context,
                    "closure metadata has no construction expression",
                ));
            };
            if *construction_count != 1 || *construction_span != closure.span {
                return Err(HirInvariantError::new(
                    &context,
                    "closure metadata and its construction expression are not one-to-one",
                ));
            }
            let TypeKind::Generated {
                identity,
                arguments,
            } = self.program.interner.kind(closure.ty).map_err(|error| {
                HirInvariantError::new(&context, format!("invalid closure type: {error}"))
            })?
            else {
                return Err(HirInvariantError::new(
                    context,
                    "closure has a non-generated concrete type",
                ));
            };
            if *identity != closure.identity
                || !arguments.iter().enumerate().all(|(position, argument)| {
                    matches!(
                        self.program.interner.kind(*argument),
                        Ok(TypeKind::GenericParameter(actual))
                            if usize::try_from(*actual).ok() == Some(position)
                    )
                })
            {
                return Err(HirInvariantError::new(
                    &context,
                    "closure type identity or inherited generic arguments are invalid",
                ));
            }
            let generic_arity = u32::try_from(arguments.len()).map_err(|_| {
                HirInvariantError::new(&context, "closure generic arity exceeds u32")
            })?;
            if closure.generic_arity != generic_arity {
                return Err(HirInvariantError::new(
                    &context,
                    "closure generic arity differs from its generated type arguments",
                ));
            }
            let inherited_positions = closure
                .generics
                .iter()
                .map(|generic| generic.position)
                .collect::<BTreeSet<_>>();
            let missing = (0..generic_arity)
                .filter(|position| !inherited_positions.contains(position))
                .collect::<Vec<_>>();
            let hidden = match missing.as_slice() {
                [] => None,
                [position] => Some(*position),
                _ => {
                    return Err(HirInvariantError::new(
                        &context,
                        "closure inherited more than one hidden generic position",
                    ));
                }
            };
            self.verify_generics(&closure.generics, generic_arity, hidden, &context)?;

            let TypeKind::Function(function) = self
                .program
                .interner
                .kind(closure.function_type)
                .map_err(|error| {
                    HirInvariantError::new(&context, format!("invalid closure signature: {error}"))
                })?
            else {
                return Err(HirInvariantError::new(
                    context,
                    "closure call signature is not a function type",
                ));
            };
            let expected_kind =
                GeneratedTypeKind::closure(function.is_async(), function.is_unsafe());
            if closure.identity.kind() != expected_kind {
                return Err(HirInvariantError::new(
                    &context,
                    "generated closure identity kind differs from its call effects",
                ));
            }
            let mut fixed = Vec::new();
            let mut variadic = None;
            for (parameter_index, parameter) in closure.parameters.iter().enumerate() {
                self.verify_type(
                    parameter.ty,
                    format!("{context} parameter {parameter_index}"),
                )?;
                let local_valid = if parameter.discard {
                    parameter.local.is_none()
                } else if let Some(local) = parameter.local {
                    self.verify_local(local, &context)?.kind() == LocalKind::ClosureParameter
                        && self.program.local_types.get(&local) == Some(&parameter.ty)
                } else {
                    false
                };
                if !local_valid || parameter.receiver {
                    return Err(HirInvariantError::new(
                        &context,
                        format!("closure parameter {parameter_index} has invalid local metadata"),
                    ));
                }
                if function.is_async()
                    && matches!(parameter.mode, ParameterMode::Mut | ParameterMode::Var)
                {
                    return Err(HirInvariantError::new(
                        &context,
                        "async closure retains an exclusive parameter across suspension",
                    ));
                }
                if let Some(element) = parameter.variadic_element {
                    if variadic.is_some()
                        || parameter_index + 1 != closure.parameters.len()
                        || parameter.mode != ParameterMode::Value
                    {
                        return Err(HirInvariantError::new(
                            &context,
                            "closure variadic parameter is not unique, final, and by value",
                        ));
                    }
                    let expected_body = self
                        .program
                        .interner
                        .kind(parameter.ty)
                        .map_err(|error| HirInvariantError::new(&context, error.to_string()))?;
                    if !matches!(
                        expected_body,
                        TypeKind::Intrinsic {
                            constructor: IntrinsicType::Array,
                            arguments,
                        } if arguments.as_slice() == [element]
                    ) {
                        return Err(HirInvariantError::new(
                            &context,
                            "closure variadic body binding is not Array[element]",
                        ));
                    }
                    variadic = Some(element);
                } else {
                    fixed.push(FunctionParameter::new(parameter.mode, parameter.ty));
                }
            }
            if function.parameters() != fixed.as_slice()
                || function.variadic() != variadic
                || closure.body.root.0 as usize >= self.program.expressions.len()
                || self.program.expressions[closure.body.root.0 as usize].ty != function.outcome()
            {
                return Err(HirInvariantError::new(
                    &context,
                    "closure parameters, body result, and call signature disagree",
                ));
            }

            let mut previous_capture = None;
            for capture in &closure.captures {
                if previous_capture.is_some_and(|previous| previous >= capture.local) {
                    return Err(HirInvariantError::new(
                        &context,
                        "closure captures are not sorted and unique",
                    ));
                }
                previous_capture = Some(capture.local);
                let local = self.verify_local(capture.local, &context)?;
                if self.program.local_types.get(&capture.local) != Some(&capture.ty) {
                    return Err(HirInvariantError::new(
                        &context,
                        format!(
                            "capture local#{} type differs from its checked binding",
                            capture.local.index()
                        ),
                    ));
                }
                if capture.mutable != self.local_is_mutable_binding(capture.local) {
                    return Err(HirInvariantError::new(
                        &context,
                        format!(
                            "capture local#{} mutability differs from its binding",
                            capture.local.index()
                        ),
                    ));
                }
                if (local.span().file() == closure.span.file()
                    && closure.span.range().start() <= local.span().range().start()
                    && local.span().range().end() <= closure.span.range().end())
                    || self.local_is_loan(capture.local)
                {
                    return Err(HirInvariantError::new(
                        &context,
                        format!(
                            "capture local#{} is not an owned outer binding",
                            capture.local.index()
                        ),
                    ));
                }
            }
            let capture_locals = closure
                .captures
                .iter()
                .map(|capture| capture.local)
                .collect::<BTreeSet<_>>();
            let expected_protocols = self.derive_closure_protocols(
                closure.body.root,
                &capture_locals,
                function.is_async(),
                &context,
            )?;
            if closure.protocols != expected_protocols {
                return Err(HirInvariantError::new(
                    &context,
                    "closure call protocols do not match independent body analysis",
                ));
            }
        }
        Ok(())
    }

    fn derive_closure_protocols(
        &self,
        root: HirExpressionId,
        captures: &BTreeSet<crate::resolve::LocalId>,
        is_async: bool,
        context: &str,
    ) -> Result<super::HirClosureProtocols, HirInvariantError> {
        let writes_capture = self.closure_body_writes_capture(root, captures, context)?;
        Ok(super::HirClosureProtocols::new(
            !writes_capture,
            !is_async || !writes_capture,
            true,
        ))
    }

    fn closure_body_writes_capture(
        &self,
        root: HirExpressionId,
        captures: &BTreeSet<crate::resolve::LocalId>,
        context: &str,
    ) -> Result<bool, HirInvariantError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let expression = self.expression(id, context)?;
            match &expression.kind {
                HirExpressionKind::Block { statements, tail } => {
                    let mut reachable = true;
                    for statement in statements {
                        if !reachable {
                            break;
                        }
                        if let HirStatement::Assignment { target, .. } = statement
                            && self.assignment_target_roots_capture(target, captures, context)?
                        {
                            return Ok(true);
                        }
                        statement_children(statement, &mut pending);
                        reachable = self.statement_may_complete(statement);
                    }
                    if reachable {
                        pending.extend(tail);
                    }
                }
                HirExpressionKind::Call {
                    callee,
                    arguments,
                    protocol,
                    ..
                } => {
                    if *protocol == super::HirCallProtocol::CallMut
                        && self
                            .expression_root_local(*callee, context)?
                            .is_some_and(|local| captures.contains(&local))
                    {
                        return Ok(true);
                    }
                    for argument in arguments {
                        if matches!(argument.mode, ParameterMode::Mut | ParameterMode::Var)
                            && self
                                .expression_root_local(argument.value, context)?
                                .is_some_and(|local| captures.contains(&local))
                        {
                            return Ok(true);
                        }
                    }
                    pending.push(*callee);
                    pending.extend(arguments.iter().map(|argument| argument.value));
                }
                HirExpressionKind::Closure(_) => {}
                _ => pending.extend(expression_children(expression)),
            }
        }
        Ok(false)
    }

    fn assignment_target_roots_capture(
        &self,
        root: &HirAssignmentTarget,
        captures: &BTreeSet<crate::resolve::LocalId>,
        context: &str,
    ) -> Result<bool, HirInvariantError> {
        let mut pending = vec![root];
        while let Some(target) = pending.pop() {
            match &target.kind {
                HirAssignmentTargetKind::Place { place, .. } => {
                    if self
                        .expression_root_local(*place, context)?
                        .is_some_and(|local| captures.contains(&local))
                    {
                        return Ok(true);
                    }
                }
                HirAssignmentTargetKind::Discard => {}
                HirAssignmentTargetKind::Tuple(items) => pending.extend(items),
            }
        }
        Ok(false)
    }

    fn expression_root_local(
        &self,
        id: HirExpressionId,
        context: &str,
    ) -> Result<Option<crate::resolve::LocalId>, HirInvariantError> {
        Ok(match &self.expression(id, context)?.kind {
            HirExpressionKind::Local(local) => Some(*local),
            HirExpressionKind::Field { base, .. }
            | HirExpressionKind::TupleField { base, .. }
            | HirExpressionKind::Index { base, .. }
            | HirExpressionKind::Slice { base, .. } => {
                self.expression_root_local(*base, context)?
            }
            _ => None,
        })
    }

    fn statement_may_complete(&self, statement: &HirStatement) -> bool {
        let flow = |id: HirExpressionId| self.program.expression_flows[id.0 as usize];
        match statement {
            HirStatement::Binding { value, .. }
            | HirStatement::Expression { value, .. }
            | HirStatement::Discard { value, .. } => flow(*value).may_complete(),
            HirStatement::Assignment { target, value, .. } => {
                let mut expressions = Vec::new();
                assignment_target_children(target, &mut expressions);
                expressions.push(*value);
                expressions
                    .into_iter()
                    .all(|expression| flow(expression).may_complete())
            }
            HirStatement::For { id, kind, body, .. } => {
                let header_completes = match kind {
                    HirForKind::Infinite => true,
                    HirForKind::Conditional { condition } => flow(*condition).may_complete(),
                    HirForKind::Iterate { source, .. } => flow(*source).may_complete(),
                };
                header_completes
                    && (!matches!(kind, HirForKind::Infinite)
                        || self.program.expression_breaks[body.0 as usize].contains(id))
            }
        }
    }

    fn local_is_loan(&self, local: crate::resolve::LocalId) -> bool {
        self.program.callables.iter().any(|callable| {
            callable.parameters.iter().any(|parameter| {
                parameter.local == Some(local) && parameter.mode != ParameterMode::Value
            })
        }) || self.program.closures.iter().any(|closure| {
            closure.parameters.iter().any(|parameter| {
                parameter.local == Some(local) && parameter.mode != ParameterMode::Value
            })
        }) || self.program.patterns.iter().any(|pattern| {
            matches!(pattern.kind, HirPatternKind::BorrowBinding(candidate) if candidate == local)
        })
    }

    fn local_is_mutable_binding(&self, local: crate::resolve::LocalId) -> bool {
        self.program.expressions.iter().any(|expression| {
            let HirExpressionKind::Block { statements, .. } = &expression.kind else {
                return false;
            };
            statements.iter().any(|statement| {
                let HirStatement::Binding {
                    mutable: true,
                    pattern,
                    ..
                } = statement
                else {
                    return false;
                };
                let mut pending = vec![*pattern];
                let mut visited = BTreeSet::new();
                while let Some(pattern) = pending.pop() {
                    if !visited.insert(pattern) {
                        continue;
                    }
                    let Some(pattern) = self.program.pattern(pattern) else {
                        continue;
                    };
                    if matches!(
                        pattern.kind,
                        HirPatternKind::Binding(candidate) if candidate == local
                    ) {
                        return true;
                    }
                    pending.extend(pattern_children(pattern));
                }
                false
            })
        })
    }

    fn verify_opaque_result(
        &self,
        callable: &super::HirCallableSignature,
        identities: &mut BTreeSet<crate::package::SymbolIdentity>,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let success = match self
            .program
            .interner
            .kind(callable.outcome)
            .map_err(|error| {
                HirInvariantError::new(context, format!("invalid callable outcome: {error}"))
            })? {
            TypeKind::Result { success, .. } => *success,
            _ => callable.outcome,
        };
        let Some(opaque) = callable.opaque_result.as_ref() else {
            if matches!(
                self.program.interner.kind(success),
                Ok(TypeKind::OpaqueResult { .. })
            ) {
                return Err(HirInvariantError::new(
                    context,
                    "opaque outcome has no declaration-owned contract",
                ));
            }
            return Ok(());
        };

        match callable.id {
            HirCallableId::Symbol(_) => {}
            HirCallableId::Member(member)
                if self.resolved.member(member).is_some_and(|member| {
                    matches!(
                        member.kind(),
                        MemberKind::InherentMethod | MemberKind::AssociatedFunction
                    )
                }) => {}
            HirCallableId::Member(_) | HirCallableId::Implementation(_) => {
                return Err(HirInvariantError::new(
                    context,
                    "trait or implementation method owns an opaque result",
                ));
            }
        }
        let TypeKind::OpaqueResult {
            identity,
            arguments,
        } = self.program.interner.kind(success).map_err(|error| {
            HirInvariantError::new(context, format!("invalid opaque success type: {error}"))
        })?
        else {
            return Err(HirInvariantError::new(
                context,
                "opaque metadata does not correspond to the top-level success type",
            ));
        };
        if identity != &opaque.identity || !identities.insert(identity.clone()) {
            return Err(HirInvariantError::new(
                context,
                "opaque identity is mismatched or duplicated",
            ));
        }
        if arguments.len() != callable.generic_arity as usize
            || !arguments.iter().enumerate().all(|(position, argument)| {
                matches!(
                    self.program.interner.kind(*argument),
                    Ok(TypeKind::GenericParameter(actual)) if *actual == position as u32
                )
            })
        {
            return Err(HirInvariantError::new(
                context,
                "opaque family does not preserve the callable generic arguments",
            ));
        }
        if opaque.bounds.is_empty() || !bounds_imply(&opaque.bounds, HirCapability::Discard) {
            return Err(HirInvariantError::new(
                context,
                "opaque bounds do not prove Discard",
            ));
        }
        let mut unique_bounds = BTreeSet::new();
        let mut call_signature = None;
        for bound in &opaque.bounds {
            if !unique_bounds.insert((bound.constructor.clone(), bound.arguments.clone())) {
                return Err(HirInvariantError::new(
                    context,
                    "opaque bounds contain a duplicate normalized contract",
                ));
            }
            if let HirTraitConstructor::Symbol(symbol) = bound.constructor {
                self.verify_symbol(symbol, &[SymbolKind::Trait], context)?;
            }
            for argument in &bound.arguments {
                self.verify_type(*argument, format!("{context} opaque bound"))?;
            }
            if let HirTraitConstructor::Prelude(name) = &bound.constructor
                && matches!(name.as_str(), "Call" | "CallMut" | "CallOnce")
            {
                let [signature] = bound.arguments.as_slice() else {
                    return Err(HirInvariantError::new(
                        context,
                        "opaque call bound does not contain one signature",
                    ));
                };
                if !matches!(
                    self.program.interner.kind(*signature),
                    Ok(TypeKind::Function(_))
                ) || call_signature.is_some_and(|previous| previous != *signature)
                {
                    return Err(HirInvariantError::new(
                        context,
                        "opaque call bounds do not use one exact function signature",
                    ));
                }
                call_signature = Some(*signature);
            }
        }
        let witness = opaque.witness.ok_or_else(|| {
            HirInvariantError::new(context, "opaque result has no concrete witness")
        })?;
        self.verify_type(witness, format!("{context} opaque witness"))?;
        if matches!(
            self.program.interner.kind(witness),
            Ok(TypeKind::Error | TypeKind::Scalar(ScalarType::Never) | TypeKind::Inference(_))
        ) {
            return Err(HirInvariantError::new(
                context,
                "opaque witness is recovery, unresolved, or Never",
            ));
        }
        let mut positions = BTreeSet::new();
        self.collect_generic_positions(witness, &mut positions)?;
        if positions
            .iter()
            .any(|position| *position >= callable.generic_arity)
        {
            return Err(HirInvariantError::new(
                context,
                "opaque witness escapes an undeclared generic parameter",
            ));
        }
        self.verify_opaque_witness_bounds(callable, opaque, witness, context)?;
        Ok(())
    }

    fn verify_opaque_witness_bounds(
        &self,
        callable: &super::HirCallableSignature,
        opaque: &super::HirOpaqueResult,
        witness: TypeId,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let mut interner = self.program.interner.clone();
        let mut assumptions = BTreeSet::new();
        for parameter in &callable.generics {
            let target = interner
                .generic_parameter(parameter.position)
                .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
            assumptions.extend(
                parameter
                    .bounds
                    .iter()
                    .map(|bound| TraitQuery::new(bound, target)),
            );
        }
        let capability_analysis =
            CapabilityAnalysis::new(self.program, self.resolved).map_err(|error| {
                HirInvariantError::new(context, format!("invalid capability graph: {error}"))
            })?;
        let capability_assumptions =
            CapabilityAssumptions::from_generics(self.program, &callable.generics);
        let mut proof = OpaqueTraitProof {
            interner,
            assumptions,
            capability_analysis,
            capability_assumptions,
            active: BTreeSet::new(),
            memo: BTreeMap::new(),
            context: context.to_owned(),
        };
        for bound in &opaque.bounds {
            let query =
                TraitQuery::from_parts(bound.constructor.clone(), bound.arguments.clone(), witness);
            if !self.prove_opaque_trait_query(&query, &mut proof)? {
                return Err(HirInvariantError::new(
                    context,
                    "opaque witness does not satisfy every published bound",
                ));
            }
        }
        Ok(())
    }

    fn prove_opaque_trait_query(
        &self,
        query: &TraitQuery,
        proof: &mut OpaqueTraitProof,
    ) -> Result<bool, HirInvariantError> {
        if let Some(proven) = proof.memo.get(query).copied() {
            return Ok(proven);
        }
        if proof.assumptions.contains(query) {
            proof.memo.insert(query.clone(), true);
            return Ok(true);
        }
        if let HirTraitConstructor::Prelude(name) = query.constructor()
            && let Some(capability) = HirCapability::from_name(name.as_str())
        {
            let proven = proof
                .capability_analysis
                .status(
                    self.program,
                    query.target(),
                    capability,
                    &proof.capability_assumptions,
                )
                .map_err(|error| {
                    HirInvariantError::new(proof.context.clone(), error.to_string())
                })?
                == super::HirCapabilityStatus::Satisfied;
            proof.memo.insert(query.clone(), proven);
            return Ok(proven);
        }
        if let Some(proven) = self.closed_call_query_proof(query, proof)? {
            proof.memo.insert(query.clone(), proven);
            return Ok(proven);
        }
        if let Some(proven) =
            self.opaque_published_query_proof(&mut proof.interner, query, &proof.context)?
        {
            proof.memo.insert(query.clone(), proven);
            return Ok(proven);
        }
        if let HirTraitConstructor::Prelude(name) = query.constructor() {
            match name.as_str() {
                "Call" | "CallMut" | "CallOnce" => {
                    proof.memo.insert(query.clone(), false);
                    return Ok(false);
                }
                "Display" | "Iterator" => {}
                _ => {
                    proof.memo.insert(query.clone(), false);
                    return Ok(false);
                }
            }
        }
        if !proof.active.insert(query.clone()) {
            return Err(HirInvariantError::new(
                &proof.context,
                "opaque bound proof re-entered an admitted trait query",
            ));
        }
        let selection =
            select_implementation(&proof.interner, &self.program.implementations, query).map_err(
                |error| match error {
                    TraitSelectionError::Ambiguous => HirInvariantError::new(
                        &proof.context,
                        "opaque bound proof found ambiguous admitted implementations",
                    ),
                    TraitSelectionError::Type(error) => {
                        HirInvariantError::new(&proof.context, error.to_string())
                    }
                },
            )?;
        let Some(selection) = selection else {
            proof.active.remove(query);
            proof.memo.insert(query.clone(), false);
            return Ok(false);
        };
        let implementation = self
            .program
            .implementation(selection.implementation())
            .ok_or_else(|| {
                HirInvariantError::new(
                    &proof.context,
                    "opaque bound selection references an absent implementation",
                )
            })?;
        let substitution = TypeSubstitution::new(selection.arguments().to_vec());
        let mut proven = true;
        for parameter in implementation.parameters() {
            let target = selection
                .arguments()
                .get(parameter.position() as usize)
                .copied()
                .ok_or_else(|| {
                    HirInvariantError::new(
                        &proof.context,
                        "opaque bound selection omitted an implementation binder",
                    )
                })?;
            for bound in parameter.bounds() {
                let arguments = bound
                    .arguments()
                    .iter()
                    .map(|argument| {
                        substitution
                            .apply(&mut proof.interner, *argument)
                            .map_err(|error| {
                                HirInvariantError::new(proof.context.clone(), error.to_string())
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let obligation =
                    TraitQuery::from_parts(bound.constructor().clone(), arguments, target);
                proven &= self.prove_opaque_trait_query(&obligation, proof)?;
            }
        }
        proof.active.remove(query);
        proof.memo.insert(query.clone(), proven);
        Ok(proven)
    }

    fn closed_call_query_proof(
        &self,
        query: &TraitQuery,
        proof: &mut OpaqueTraitProof,
    ) -> Result<Option<bool>, HirInvariantError> {
        let HirTraitConstructor::Prelude(name) = query.constructor() else {
            return Ok(None);
        };
        let required = match name.as_str() {
            "Call" => super::HirCallProtocol::Call,
            "CallMut" => super::HirCallProtocol::CallMut,
            "CallOnce" => super::HirCallProtocol::CallOnce,
            _ => return Ok(None),
        };
        let [signature] = query.arguments() else {
            return Ok(Some(false));
        };
        if !matches!(proof.interner.kind(*signature), Ok(TypeKind::Function(_))) {
            return Ok(Some(false));
        }
        match proof
            .interner
            .kind(query.target())
            .map_err(|error| HirInvariantError::new(&proof.context, error.to_string()))?
        {
            TypeKind::Function(_) => return Ok(Some(*signature == query.target())),
            TypeKind::Generated {
                identity,
                arguments,
            } => {
                if let Some(closure) = self.program.closure_by_identity(identity) {
                    let actual = TypeSubstitution::new(arguments.clone())
                        .apply(&mut proof.interner, closure.function_type)
                        .map_err(|error| {
                            HirInvariantError::new(&proof.context, error.to_string())
                        })?;
                    return Ok(Some(
                        actual == *signature && closure.protocols.supports(required),
                    ));
                }
            }
            _ => {}
        }

        let available = proof
            .assumptions
            .iter()
            .filter(|assumption| assumption.target() == query.target())
            .filter_map(|assumption| {
                let HirTraitConstructor::Prelude(name) = assumption.constructor() else {
                    return None;
                };
                let protocol = match name.as_str() {
                    "Call" => super::HirCallProtocol::Call,
                    "CallMut" => super::HirCallProtocol::CallMut,
                    "CallOnce" => super::HirCallProtocol::CallOnce,
                    _ => return None,
                };
                (assumption.arguments() == [*signature]).then_some(protocol)
            })
            .collect::<BTreeSet<_>>();
        if available.is_empty() {
            return Ok(None);
        }
        let has = |protocol| available.contains(&protocol);
        let discard = proof
            .capability_analysis
            .status(
                self.program,
                query.target(),
                HirCapability::Discard,
                &proof.capability_assumptions,
            )
            .map_err(|error| HirInvariantError::new(&proof.context, error.to_string()))?
            == super::HirCapabilityStatus::Satisfied;
        Ok(Some(match required {
            super::HirCallProtocol::Call => has(super::HirCallProtocol::Call),
            super::HirCallProtocol::CallMut => {
                has(super::HirCallProtocol::Call) || has(super::HirCallProtocol::CallMut)
            }
            super::HirCallProtocol::CallOnce => {
                has(super::HirCallProtocol::CallOnce)
                    || (discard
                        && (has(super::HirCallProtocol::Call)
                            || has(super::HirCallProtocol::CallMut)))
            }
        }))
    }

    fn opaque_published_query_proof(
        &self,
        interner: &mut TypeInterner,
        query: &TraitQuery,
        context: &str,
    ) -> Result<Option<bool>, HirInvariantError> {
        let TypeKind::OpaqueResult {
            identity,
            arguments,
        } = interner
            .kind(query.target())
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
            .clone()
        else {
            return Ok(None);
        };
        let opaque = self.program.opaque_result(&identity).ok_or_else(|| {
            HirInvariantError::new(context, "opaque query has no declaration-owned contract")
        })?;
        let substitution = TypeSubstitution::new(arguments);
        let published = opaque
            .bounds
            .iter()
            .map(|bound| {
                let arguments = bound
                    .arguments
                    .iter()
                    .map(|argument| {
                        substitution
                            .apply(interner, *argument)
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(TraitQuery::from_parts(
                    bound.constructor.clone(),
                    arguments,
                    query.target(),
                ))
            })
            .collect::<Result<Vec<_>, HirInvariantError>>()?;
        if published.contains(query) {
            return Ok(Some(true));
        }
        let HirTraitConstructor::Prelude(required) = query.constructor() else {
            return Ok(Some(false));
        };
        let has = |name: &str, arguments: &[TypeId]| {
            published.iter().any(|bound| {
                matches!(
                    bound.constructor(),
                    HirTraitConstructor::Prelude(candidate) if candidate.as_str() == name
                ) && bound.arguments() == arguments
            })
        };
        let discard = has("Discard", &[]) || has("Copy", &[]) || has("Key", &[]);
        let proven = match required.as_str() {
            "Discard" => discard,
            "Copy" => has("Copy", &[]) || has("Key", &[]),
            "Equatable" => has("Equatable", &[]) || has("Key", &[]),
            "CallMut" => has("CallMut", query.arguments()) || has("Call", query.arguments()),
            "CallOnce" => {
                has("CallOnce", query.arguments())
                    || (discard
                        && (has("CallMut", query.arguments()) || has("Call", query.arguments())))
            }
            _ => false,
        };
        Ok(Some(proven))
    }

    fn verify_generics(
        &self,
        generics: &[HirGenericParameter],
        generic_arity: u32,
        hidden_position: Option<u32>,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        if hidden_position.is_some_and(|position| position >= generic_arity) {
            return Err(HirInvariantError::new(
                context,
                "hidden generic position is outside the callable arity",
            ));
        }
        let expected_len = usize::try_from(generic_arity)
            .ok()
            .and_then(|arity| arity.checked_sub(usize::from(hidden_position.is_some())))
            .ok_or_else(|| HirInvariantError::new(context, "invalid generic arity"))?;
        if generics.len() != expected_len {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "generic parameter count {} does not match arity {generic_arity}",
                    generics.len()
                ),
            ));
        }
        let expected_positions = (0..generic_arity)
            .filter(|position| Some(*position) != hidden_position)
            .collect::<Vec<_>>();
        for (generic, expected) in generics.iter().zip(expected_positions) {
            if generic.position != expected {
                return Err(HirInvariantError::new(
                    context,
                    format!(
                        "generic local#{} has position {}, expected {expected}",
                        generic.local.index(),
                        generic.position
                    ),
                ));
            }
            let local = self.verify_local(generic.local, context)?;
            if local.kind() != LocalKind::GenericParameter {
                return Err(HirInvariantError::new(
                    context,
                    format!("local#{} is not a generic parameter", generic.local.index()),
                ));
            }
            let mut call_signature = None;
            for bound in &generic.bounds {
                if let super::HirTraitConstructor::Symbol(symbol) = bound.constructor {
                    self.verify_symbol(symbol, &[SymbolKind::Trait], context)?;
                }
                for argument in &bound.arguments {
                    self.verify_type(*argument, format!("{context} generic bound"))?;
                }
                if let HirTraitConstructor::Prelude(name) = &bound.constructor
                    && matches!(name.as_str(), "Call" | "CallMut" | "CallOnce")
                {
                    let [signature] = bound.arguments.as_slice() else {
                        return Err(HirInvariantError::new(
                            context,
                            "call bound does not contain one signature",
                        ));
                    };
                    if !matches!(
                        self.program.interner.kind(*signature),
                        Ok(TypeKind::Function(_))
                    ) || call_signature.is_some_and(|previous| previous != *signature)
                    {
                        return Err(HirInvariantError::new(
                            context,
                            "call bounds do not use one exact function signature",
                        ));
                    }
                    call_signature = Some(*signature);
                }
            }
        }
        Ok(())
    }

    fn trait_self_position(&self, callable: HirCallableId) -> Option<u32> {
        let HirCallableId::Member(member) = callable else {
            return None;
        };
        let member = self.resolved.member(member)?;
        if !matches!(
            member.kind(),
            MemberKind::TraitMethod | MemberKind::TraitAssociatedFunction
        ) {
            return None;
        }
        let MemberOwner::Type(owner) = member.owner() else {
            return None;
        };
        let declaration = self.program.declaration(owner)?;
        u32::try_from(declaration.parameters.len()).ok()
    }

    fn verify_annotations_and_locals(&self) -> Result<(), HirInvariantError> {
        for ((file, start, end), ty) in &self.program.annotations {
            self.verify_type(
                *ty,
                format!("type annotation in {file} at bytes {start}..{end}"),
            )?;
        }
        for (local, ty) in &self.program.local_types {
            self.verify_local(*local, "local type table")?;
            self.verify_type(*ty, format!("local#{}", local.index()))?;
        }
        Ok(())
    }

    fn verify_member_references(&self) -> Result<(), HirInvariantError> {
        for reference in &self.program.member_references {
            self.verify_member(reference.member, &[], "member reference")?;
        }
        Ok(())
    }

    fn verify_patterns(&self) -> Result<(), HirInvariantError> {
        for (index, pattern) in self.program.patterns.iter().enumerate() {
            let id = HirPatternId(index as u32);
            let context = format!("pattern#{}", id.index());
            self.verify_type(pattern.ty, format!("{context} type"))?;
            if matches!(pattern.kind, HirPatternKind::Recovery) {
                return Err(HirInvariantError::new(
                    context,
                    "recovery pattern escaped a successful semantic check",
                ));
            }
            for child in pattern_children(pattern) {
                self.pattern_before(child, id, &context)?;
            }
            self.verify_pattern_names(pattern, &context)?;
        }
        Ok(())
    }

    fn verify_pattern_names(
        &self,
        pattern: &HirPattern,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match &pattern.kind {
            HirPatternKind::Binding(local) | HirPatternKind::BorrowBinding(local) => {
                self.verify_local(*local, context)?;
                let ty = self.program.local_types.get(local).ok_or_else(|| {
                    HirInvariantError::new(
                        context,
                        format!("local#{} has no checked type", local.index()),
                    )
                })?;
                if *ty != pattern.ty {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "local#{} has {}, pattern has {}",
                            local.index(),
                            ty,
                            pattern.ty
                        ),
                    ));
                }
            }
            HirPatternKind::Newtype { constructor, .. } => {
                self.verify_symbol(*constructor, &[SymbolKind::Type], context)?
            }
            HirPatternKind::Variant { variant, .. } => {
                self.verify_member(*variant, &[MemberKind::EnumVariant], context)?;
            }
            HirPatternKind::Record { owner, fields, .. } => {
                self.verify_symbol(*owner, &[SymbolKind::Type], context)?;
                for field in fields {
                    self.verify_member(field.member, &[MemberKind::RecordField], context)?;
                }
            }
            HirPatternKind::UnionMember { member, .. } => {
                self.verify_type(*member, format!("{context} union member"))?;
            }
            HirPatternKind::Recovery
            | HirPatternKind::Wildcard
            | HirPatternKind::Literal(_)
            | HirPatternKind::Tuple(_)
            | HirPatternKind::OptionSome(_)
            | HirPatternKind::OptionNone
            | HirPatternKind::ResultOk(_)
            | HirPatternKind::ResultErr(_)
            | HirPatternKind::Array { .. } => {}
        }
        Ok(())
    }

    fn collect_loops(&self) -> Result<BTreeSet<super::HirLoopId>, HirInvariantError> {
        let mut loops = BTreeSet::new();
        for expression in &self.program.expressions {
            let HirExpressionKind::Block { statements, .. } = &expression.kind else {
                continue;
            };
            for statement in statements {
                if let HirStatement::For { id, .. } = statement
                    && !loops.insert(*id)
                {
                    return Err(HirInvariantError::new(
                        format!("loop#{}", id.index()),
                        "loop ID is duplicated",
                    ));
                }
            }
        }
        Ok(loops)
    }

    fn verify_expressions(
        &self,
        loops: &BTreeSet<super::HirLoopId>,
    ) -> Result<(), HirInvariantError> {
        for (index, expression) in self.program.expressions.iter().enumerate() {
            let id = HirExpressionId(index as u32);
            let context = format!("expression#{}", id.index());
            self.verify_type(expression.ty, format!("{context} type"))?;
            if matches!(expression.kind, HirExpressionKind::Recovery) {
                return Err(HirInvariantError::new(
                    context,
                    "recovery expression escaped a successful semantic check",
                ));
            }
            for child in expression_children(expression) {
                self.expression_before(child, id, &context)?;
            }
            self.verify_expression_category(id, expression, &context)?;
            self.verify_expression_names(expression, &context)?;
            self.verify_expression_loops(id, expression, loops, &context)?;
        }
        Ok(())
    }

    fn verify_expression_category(
        &self,
        id: HirExpressionId,
        expression: &HirExpression,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let expected = match expression.kind {
            HirExpressionKind::Local(_) | HirExpressionKind::Receiver => {
                Some(HirValueCategory::Place)
            }
            HirExpressionKind::Field { base, .. } | HirExpressionKind::TupleField { base, .. } => {
                Some(self.expression(base, context)?.category)
            }
            HirExpressionKind::Index { base, .. } | HirExpressionKind::Slice { base, .. } => {
                if expression.category == HirValueCategory::Place
                    && self.expression(base, context)?.category != HirValueCategory::Place
                {
                    return Err(HirInvariantError::new(
                        context,
                        "a place projection starts from a value",
                    ));
                }
                None
            }
            HirExpressionKind::Recovery => return Ok(()),
            _ => Some(HirValueCategory::Value),
        };
        if let Some(expected) = expected
            && expression.category != expected
        {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "category is {}, expected {} for this expression kind",
                    category_name(expression.category),
                    category_name(expected)
                ),
            ));
        }
        if expression.category == HirValueCategory::Place
            && matches!(
                expression.kind,
                HirExpressionKind::Return { .. }
                    | HirExpressionKind::Fail { .. }
                    | HirExpressionKind::Break { .. }
                    | HirExpressionKind::Continue { .. }
            )
        {
            return Err(HirInvariantError::new(
                format!("expression#{}", id.index()),
                "control transfer cannot be a place",
            ));
        }
        Ok(())
    }

    fn verify_expression_names(
        &self,
        expression: &HirExpression,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match &expression.kind {
            HirExpressionKind::Local(local) => {
                self.verify_local(*local, context)?;
                let ty = self.program.local_types.get(local).ok_or_else(|| {
                    HirInvariantError::new(
                        context,
                        format!("local#{} has no checked type", local.index()),
                    )
                })?;
                if *ty != expression.ty {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "local#{} has {}, expression has {}",
                            local.index(),
                            ty,
                            expression.ty
                        ),
                    ));
                }
            }
            HirExpressionKind::Constant(symbol) => {
                self.verify_symbol(*symbol, &[SymbolKind::Constant], context)?;
                let constant = self.program.constants.get(symbol).ok_or_else(|| {
                    HirInvariantError::new(
                        context,
                        format!(
                            "constant symbol#{} is absent from typed HIR",
                            symbol.index()
                        ),
                    )
                })?;
                if constant.ty != Some(expression.ty) {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "constant symbol#{} has a different checked type",
                            symbol.index()
                        ),
                    ));
                }
            }
            HirExpressionKind::Function(callable) => {
                let signature = self.verify_callable_id(*callable, context)?;
                if signature.generic_arity != 0 {
                    return Err(HirInvariantError::new(
                        context,
                        "generic named function escaped without one complete specialization",
                    ));
                }
                if expression.ty != signature.function_type {
                    return Err(HirInvariantError::new(
                        context,
                        "function value type differs from its callable signature",
                    ));
                }
            }
            HirExpressionKind::SpecializedFunction {
                callable,
                arguments,
            } => {
                let signature = self.verify_callable_id(*callable, context)?;
                if arguments.len() != signature.generic_arity as usize {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "specialization has {} arguments for {} generic parameters",
                            arguments.len(),
                            signature.generic_arity
                        ),
                    ));
                }
                for argument in arguments {
                    self.verify_type(*argument, format!("{context} specialization argument"))?;
                }
                let mut interner = self.program.interner.clone();
                let expected = TypeSubstitution::new(arguments.clone())
                    .apply(&mut interner, signature.function_type)
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
                if expression.ty != expected {
                    return Err(HirInvariantError::new(
                        context,
                        "specialized function value type differs from its exact substituted signature",
                    ));
                }
            }
            HirExpressionKind::PreludeTraitFunction { method, arguments } => {
                let generic_arity = method.generic_arity() as usize;
                if !arguments.is_empty() && arguments.len() != generic_arity {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "prelude trait specialization has {} arguments for {generic_arity} generic parameters",
                            arguments.len()
                        ),
                    ));
                }
                for argument in arguments {
                    self.verify_type(
                        *argument,
                        format!("{context} prelude trait specialization argument"),
                    )?;
                }
                let mut interner = self.program.interner.clone();
                let complete_arguments = if arguments.is_empty() {
                    (0..method.generic_arity())
                        .map(|position| {
                            interner
                                .generic_parameter(position)
                                .map_err(|error| HirInvariantError::new(context, error.to_string()))
                        })
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    arguments.clone()
                };
                let expected = method
                    .function_type(&mut interner, &complete_arguments)
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                    .ok_or_else(|| {
                        HirInvariantError::new(
                            context,
                            "prelude trait function has an invalid specialization arity",
                        )
                    })?;
                if expression.ty != expected {
                    return Err(HirInvariantError::new(
                        context,
                        "prelude trait function type differs from its closed contract",
                    ));
                }
            }
            HirExpressionKind::Closure(closure) => {
                let closure = self.program.closure(*closure).ok_or_else(|| {
                    HirInvariantError::new(context, "closure expression has no closure metadata")
                })?;
                if expression.ty != closure.ty {
                    return Err(HirInvariantError::new(
                        context,
                        "closure expression type differs from its concrete closure type",
                    ));
                }
            }
            HirExpressionKind::Newtype { constructor, .. } => {
                self.verify_symbol(*constructor, &[SymbolKind::Type], context)?
            }
            HirExpressionKind::Record { owner, fields } => {
                self.verify_symbol(*owner, &[SymbolKind::Type], context)?;
                self.verify_record_field_values(fields, MemberKind::RecordField, context)?;
            }
            HirExpressionKind::Variant { variant, payload } => {
                self.verify_member(*variant, &[MemberKind::EnumVariant], context)?;
                self.verify_variant_value(payload, context)?;
            }
            HirExpressionKind::RecordUpdate { fields, .. } => {
                self.verify_record_field_values(fields, MemberKind::RecordField, context)?;
            }
            HirExpressionKind::Field { member, .. } => {
                self.verify_member(
                    *member,
                    &[
                        MemberKind::RecordField,
                        MemberKind::NewtypeValue,
                        MemberKind::VariantField,
                    ],
                    context,
                )?;
            }
            HirExpressionKind::Call {
                callee,
                arguments,
                signature,
                protocol,
            } => {
                let callee = self.expression(*callee, context)?;
                let TypeKind::Function(function) = self
                    .program
                    .interner
                    .kind(*signature)
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                else {
                    return Err(HirInvariantError::new(
                        context,
                        "call metadata does not carry a function signature",
                    ));
                };
                if function.is_async() || function.is_unsafe() {
                    return Err(HirInvariantError::new(
                        context,
                        "effectful call reached the synchronous safe HIR call operation",
                    ));
                }
                if expression.ty != function.outcome() {
                    return Err(HirInvariantError::new(
                        context,
                        "call result differs from its recorded signature outcome",
                    ));
                }
                let concrete_valid = match self
                    .program
                    .interner
                    .kind(callee.ty)
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                {
                    TypeKind::Function(_) => {
                        callee.ty == *signature && *protocol == super::HirCallProtocol::Call
                    }
                    TypeKind::Generated {
                        identity,
                        arguments: generic_arguments,
                    } => {
                        if let Some(closure) = self.program.closure_by_identity(identity) {
                            let mut interner = self.program.interner.clone();
                            let actual = TypeSubstitution::new(generic_arguments.clone())
                                .apply(&mut interner, closure.function_type)
                                .map_err(|error| {
                                    HirInvariantError::new(context, error.to_string())
                                })?;
                            actual == *signature && closure.protocols.supports(*protocol)
                        } else {
                            false
                        }
                    }
                    TypeKind::GenericParameter(_) | TypeKind::OpaqueResult { .. } => true,
                    _ => false,
                };
                if !concrete_valid {
                    return Err(HirInvariantError::new(
                        context,
                        "call signature or protocol is not provided by its callee type",
                    ));
                }
                for argument in arguments {
                    if argument.target == super::HirCallArgumentTarget::Invalid {
                        return Err(HirInvariantError::new(
                            context,
                            "call retains an invalid argument association",
                        ));
                    }
                    let (expected_mode, expected_type) = match argument.target {
                        super::HirCallArgumentTarget::Receiver => function
                            .parameters()
                            .first()
                            .map(|parameter| (parameter.mode(), parameter.ty())),
                        super::HirCallArgumentTarget::Fixed(index) => function
                            .parameters()
                            .get(index as usize)
                            .map(|parameter| (parameter.mode(), parameter.ty())),
                        super::HirCallArgumentTarget::VariadicElement => {
                            function.variadic().map(|ty| (ParameterMode::Value, ty))
                        }
                        super::HirCallArgumentTarget::VariadicSpread => function
                            .variadic()
                            .map(|element| {
                                let mut interner = self.program.interner.clone();
                                interner
                                    .intrinsic(IntrinsicType::Array, vec![element])
                                    .map(|ty| (ParameterMode::Value, ty))
                            })
                            .transpose()
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))?,
                        super::HirCallArgumentTarget::Invalid => None,
                    }
                    .ok_or_else(|| {
                        HirInvariantError::new(
                            context,
                            "call argument target is absent from its recorded signature",
                        )
                    })?;
                    let value = self.expression(argument.value, context)?;
                    if argument.mode != expected_mode || value.ty != expected_type {
                        return Err(HirInvariantError::new(
                            context,
                            "call argument mode or type differs from its signature slot",
                        ));
                    }
                }
            }
            HirExpressionKind::PreludePanic { message } => {
                let message = self.expression(*message, context)?;
                if message.ty != self.program.interner.scalar(ScalarType::String)
                    || expression.ty != self.program.interner.scalar(ScalarType::Never)
                {
                    return Err(HirInvariantError::new(
                        context,
                        "prelude panic requires a String message and has type Never",
                    ));
                }
            }
            HirExpressionKind::PreludeAssert {
                condition,
                condition_repr,
                message_parts,
            } => {
                let condition = self.expression(*condition, context)?;
                let bool_type = self.program.interner.scalar(ScalarType::Bool);
                let string_type = self.program.interner.scalar(ScalarType::String);
                if condition.ty != bool_type
                    || expression.ty != self.program.interner.scalar(ScalarType::Unit)
                    || condition_repr.is_empty()
                {
                    return Err(HirInvariantError::new(
                        context,
                        "prelude assert requires a Bool condition and has type Unit",
                    ));
                }
                for part in message_parts {
                    let part_expression = self.expression(part.value(), context)?;
                    if part.is_spread() {
                        if !matches!(
                            self.program.interner.kind(part_expression.ty),
                            Ok(TypeKind::Intrinsic {
                                constructor: IntrinsicType::Array,
                                arguments,
                            }) if arguments.as_slice() == [string_type]
                        ) {
                            return Err(HirInvariantError::new(
                                context,
                                "spread assert message part is not Array[String]",
                            ));
                        }
                    } else if part_expression.ty != string_type {
                        return Err(HirInvariantError::new(
                            context,
                            "assert message part is not String",
                        ));
                    }
                }
            }
            HirExpressionKind::BootstrapHostCall {
                function,
                arguments,
            } => {
                if !matches!(function, super::HirBootstrapHostFunction::ConsolePrint)
                    || arguments.len() != 1
                    || self.expression(arguments[0], context)?.ty
                        != self.program.interner.scalar(ScalarType::String)
                    || expression.ty != self.program.interner.scalar(ScalarType::Unit)
                {
                    return Err(HirInvariantError::new(
                        context,
                        "bootstrap console print requires one String and has type Unit",
                    ));
                }
            }
            HirExpressionKind::Block { statements, .. } => {
                for statement in statements {
                    self.verify_statement(statement, context)?;
                }
            }
            HirExpressionKind::Coerce { kind, value } => {
                let actual = self.expression(*value, context)?.ty;
                let valid = match kind {
                    crate::types::Assignability::Opaque => {
                        let mut interner = self.program.interner.clone();
                        self.program
                            .opaque_coercion_matches(&mut interner, actual, expression.ty)
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                    }
                    crate::types::Assignability::CallableErasure => {
                        self.callable_erasure_matches(actual, expression.ty, context)?
                    }
                    _ => {
                        self.program
                            .interner
                            .assignability(actual, expression.ty)
                            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                            == Some(*kind)
                    }
                };
                if !valid || *kind == crate::types::Assignability::Exact {
                    return Err(HirInvariantError::new(
                        context,
                        "coercion does not match its closed semantic relation",
                    ));
                }
            }
            HirExpressionKind::Recovery
            | HirExpressionKind::Literal(_)
            | HirExpressionKind::InterpolatedString { .. }
            | HirExpressionKind::Receiver
            | HirExpressionKind::Tuple(_)
            | HirExpressionKind::Array(_)
            | HirExpressionKind::Map { .. }
            | HirExpressionKind::Set(_)
            | HirExpressionKind::NumericConversion { .. }
            | HirExpressionKind::Prefix { .. }
            | HirExpressionKind::Binary { .. }
            | HirExpressionKind::Range { .. }
            | HirExpressionKind::Contains { .. }
            | HirExpressionKind::TupleField { .. }
            | HirExpressionKind::Index { .. }
            | HirExpressionKind::Slice { .. }
            | HirExpressionKind::OptionSome { .. }
            | HirExpressionKind::ResultOk { .. }
            | HirExpressionKind::ResultErr { .. }
            | HirExpressionKind::PropagateOption { .. }
            | HirExpressionKind::PropagateResult { .. }
            | HirExpressionKind::If { .. }
            | HirExpressionKind::Match { .. }
            | HirExpressionKind::Return { .. }
            | HirExpressionKind::Fail { .. }
            | HirExpressionKind::Break { .. }
            | HirExpressionKind::Continue { .. } => {}
        }
        Ok(())
    }

    fn callable_erasure_matches(
        &self,
        actual: TypeId,
        expected: TypeId,
        context: &str,
    ) -> Result<bool, HirInvariantError> {
        if !matches!(
            self.program.interner.kind(expected),
            Ok(TypeKind::Function(_))
        ) {
            return Ok(false);
        }
        let TypeKind::Generated {
            identity,
            arguments,
        } = self
            .program
            .interner
            .kind(actual)
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
        else {
            return Ok(false);
        };
        let Some(closure) = self.program.closure_by_identity(identity) else {
            return Ok(false);
        };
        let mut interner = self.program.interner.clone();
        let signature = TypeSubstitution::new(arguments.clone())
            .apply(&mut interner, closure.function_type)
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
        Ok(signature == expected && closure.protocols.supports(super::HirCallProtocol::Call))
    }

    fn verify_statement(
        &self,
        statement: &HirStatement,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let HirStatement::For {
            kind:
                HirForKind::Iterate {
                    pattern,
                    source,
                    protocol,
                },
            ..
        } = statement
        else {
            return Ok(());
        };
        let source = self.expression(*source, context)?;
        let pattern_id = *pattern;
        let pattern = self.program.pattern(pattern_id).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("iterator references unknown pattern#{}", pattern_id.index()),
            )
        })?;
        let expected_element = match protocol {
            HirIterationProtocol::Intrinsic { cursor } => {
                self.verify_type(*cursor, format!("{context} intrinsic cursor"))?;
                let expected_mode = if self.pattern_contains_borrow(pattern_id, context)? {
                    CursorMode::Ref
                } else {
                    CursorMode::Own
                };
                match self.program.interner.kind(*cursor) {
                    Ok(TypeKind::Cursor { mode, collection })
                        if *mode == expected_mode && *collection == source.ty => {}
                    Ok(_) | Err(_) => {
                        return Err(HirInvariantError::new(
                            context,
                            "intrinsic iterator protocol has an inconsistent concrete cursor",
                        ));
                    }
                }
                match self.program.interner.kind(source.ty) {
                    Ok(TypeKind::Intrinsic {
                        constructor:
                            IntrinsicType::Array | IntrinsicType::Set | IntrinsicType::Range,
                        arguments,
                    }) => Some(arguments[0]),
                    Ok(TypeKind::Intrinsic {
                        constructor: IntrinsicType::Map,
                        arguments,
                    }) => {
                        let mut interner = self.program.interner.clone();
                        Some(
                            interner.tuple(arguments.clone()).map_err(|error| {
                                HirInvariantError::new(context, error.to_string())
                            })?,
                        )
                    }
                    Ok(TypeKind::Scalar(ScalarType::String)) => {
                        Some(self.program.interner.scalar(ScalarType::Char))
                    }
                    Ok(TypeKind::Error) => None,
                    Ok(_) | Err(_) => {
                        return Err(HirInvariantError::new(
                            context,
                            "intrinsic iterator protocol has a non-intrinsic source",
                        ));
                    }
                }
            }
            HirIterationProtocol::Trait {
                element,
                function_type,
            } => {
                self.verify_type(*element, format!("{context} iterator element"))?;
                self.verify_type(*function_type, format!("{context} iterator function"))?;
                let mut interner = self.program.interner.clone();
                let expected = super::HirPreludeTraitMethod::IteratorNext
                    .function_type(&mut interner, &[*element, source.ty])
                    .map_err(|error| HirInvariantError::new(context, error.to_string()))?
                    .expect("Iterator.next receives its element and Self types");
                if expected != *function_type {
                    return Err(HirInvariantError::new(
                        context,
                        "trait iterator protocol does not match the closed Iterator.next contract",
                    ));
                }
                Some(*element)
            }
        };
        if let Some(expected_element) = expected_element
            && pattern.ty != expected_element
        {
            return Err(HirInvariantError::new(
                context,
                "iterator pattern type differs from its element type",
            ));
        }
        Ok(())
    }

    fn pattern_contains_borrow(
        &self,
        root: HirPatternId,
        context: &str,
    ) -> Result<bool, HirInvariantError> {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let pattern = self.program.pattern(id).ok_or_else(|| {
                HirInvariantError::new(
                    context,
                    format!("iterator references unknown pattern#{}", id.index()),
                )
            })?;
            if matches!(pattern.kind(), HirPatternKind::BorrowBinding(_)) {
                return Ok(true);
            }
            pending.extend(pattern_children(pattern));
        }
        Ok(false)
    }

    fn verify_record_field_values(
        &self,
        fields: &[super::HirRecordFieldValue],
        kind: MemberKind,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let mut seen = BTreeSet::new();
        for field in fields {
            self.verify_member(field.member, &[kind], context)?;
            if !seen.insert(field.member) {
                return Err(HirInvariantError::new(
                    context,
                    format!("field member#{} is initialized twice", field.member.index()),
                ));
            }
        }
        Ok(())
    }

    fn verify_variant_value(
        &self,
        payload: &HirVariantValue,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        if let HirVariantValue::Record(fields) = payload {
            self.verify_record_field_values(fields, MemberKind::VariantField, context)?;
        }
        Ok(())
    }

    fn verify_expression_loops(
        &self,
        id: HirExpressionId,
        expression: &HirExpression,
        loops: &BTreeSet<super::HirLoopId>,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match expression.kind {
            HirExpressionKind::Break { target } | HirExpressionKind::Continue { target } => {
                let target = target.ok_or_else(|| {
                    HirInvariantError::new(context, "control transfer has no resolved loop target")
                })?;
                if !loops.contains(&target) {
                    return Err(HirInvariantError::new(
                        context,
                        format!("control transfer targets unknown loop#{}", target.index()),
                    ));
                }
                if self.program.expression_flows[id.0 as usize] != HirFlow::Diverges {
                    return Err(HirInvariantError::new(
                        context,
                        "resolved loop transfer is not marked as diverging",
                    ));
                }
            }
            HirExpressionKind::Return { .. }
            | HirExpressionKind::Fail { .. }
            | HirExpressionKind::PreludePanic { .. } => {
                if self.program.expression_flows[id.0 as usize] != HirFlow::Diverges {
                    return Err(HirInvariantError::new(
                        context,
                        "function transfer is not marked as diverging",
                    ));
                }
            }
            _ => {}
        }
        let breaks = &self.program.expression_breaks[id.0 as usize];
        if !breaks.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(HirInvariantError::new(
                context,
                "break summary is not sorted and duplicate-free",
            ));
        }
        if let Some(target) = breaks.iter().find(|target| !loops.contains(target)) {
            return Err(HirInvariantError::new(
                context,
                format!("break summary contains unknown loop#{}", target.index()),
            ));
        }
        Ok(())
    }

    fn verify_bodies(&self) -> Result<(), HirInvariantError> {
        for (callable, body) in &self.program.bodies {
            let context = format!("{} body", callable_context(*callable));
            let signature = self.verify_callable_id(*callable, &context)?;
            if signature.body_source.is_none() {
                return Err(HirInvariantError::new(
                    context,
                    "checked body belongs to a body-less callable",
                ));
            }
            self.expression(body.root, &context)?;
        }
        Ok(())
    }

    fn verify_call_protocol_contracts(&self) -> Result<(), HirInvariantError> {
        let analysis = CapabilityAnalysis::new(self.program, self.resolved).map_err(|error| {
            HirInvariantError::new(
                "call protocols",
                format!("invalid capability graph: {error}"),
            )
        })?;
        for (callable, body) in &self.program.bodies {
            let signature = self.program.callable(*callable).ok_or_else(|| {
                HirInvariantError::new("call protocols", "callable body has no signature")
            })?;
            let assumptions = self.trait_assumptions(&signature.generics, "call protocols")?;
            let capabilities =
                CapabilityAssumptions::from_generics(self.program, &signature.generics);
            let exclusive_parameters = signature
                .parameters
                .iter()
                .filter(|parameter| {
                    matches!(parameter.mode, ParameterMode::Mut | ParameterMode::Var)
                })
                .filter_map(|parameter| parameter.local)
                .collect::<BTreeSet<_>>();
            let mutable_receiver = signature.parameters.iter().any(|parameter| {
                parameter.receiver
                    && matches!(parameter.mode, ParameterMode::Mut | ParameterMode::Var)
            });
            let context = format!("{} call protocols", callable_context(*callable));
            self.verify_call_protocol_tree(
                body.root,
                CallProtocolVerification {
                    assumptions: &assumptions,
                    capabilities: &capabilities,
                    analysis: &analysis,
                    exclusive_parameters: &exclusive_parameters,
                    mutable_receiver,
                    context: &context,
                },
            )?;
        }
        for closure in &self.program.closures {
            let assumptions = self.trait_assumptions(
                &closure.generics,
                &format!("closure#{} call protocols", closure.id.index()),
            )?;
            let capabilities =
                CapabilityAssumptions::from_generics(self.program, &closure.generics);
            let exclusive_parameters = closure
                .parameters
                .iter()
                .filter(|parameter| {
                    matches!(parameter.mode, ParameterMode::Mut | ParameterMode::Var)
                })
                .filter_map(|parameter| parameter.local)
                .collect::<BTreeSet<_>>();
            let context = format!("closure#{} call protocols", closure.id.index());
            self.verify_call_protocol_tree(
                closure.body.root,
                CallProtocolVerification {
                    assumptions: &assumptions,
                    capabilities: &capabilities,
                    analysis: &analysis,
                    exclusive_parameters: &exclusive_parameters,
                    mutable_receiver: false,
                    context: &context,
                },
            )?;
        }
        Ok(())
    }

    fn trait_assumptions(
        &self,
        generics: &[HirGenericParameter],
        context: &str,
    ) -> Result<BTreeSet<TraitQuery>, HirInvariantError> {
        let mut interner = self.program.interner.clone();
        let mut assumptions = BTreeSet::new();
        for parameter in generics {
            let target = interner
                .generic_parameter(parameter.position)
                .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
            assumptions.extend(
                parameter
                    .bounds
                    .iter()
                    .map(|bound| TraitQuery::new(bound, target)),
            );
        }
        Ok(assumptions)
    }

    fn verify_call_protocol_tree(
        &self,
        root: HirExpressionId,
        verification: CallProtocolVerification<'_>,
    ) -> Result<(), HirInvariantError> {
        let CallProtocolVerification {
            assumptions,
            capabilities,
            analysis,
            exclusive_parameters,
            mutable_receiver,
            context,
        } = verification;
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let expression = self.expression(id, context)?;
            if let HirExpressionKind::Call {
                callee,
                signature,
                protocol,
                ..
            } = &expression.kind
            {
                let callee_type = self.expression(*callee, context)?.ty;
                let available = self.available_call_protocols(
                    callee_type,
                    *signature,
                    assumptions,
                    capabilities,
                    analysis,
                    context,
                )?;
                let expected = if available.supports(super::HirCallProtocol::Call) {
                    Some(super::HirCallProtocol::Call)
                } else if available.supports(super::HirCallProtocol::CallMut)
                    && self.call_mut_place_is_available(
                        *callee,
                        exclusive_parameters,
                        mutable_receiver,
                        context,
                    )?
                {
                    Some(super::HirCallProtocol::CallMut)
                } else if available.supports(super::HirCallProtocol::CallOnce) {
                    Some(super::HirCallProtocol::CallOnce)
                } else {
                    None
                };
                if expected != Some(*protocol) {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "call expression#{} records {protocol:?}, expected {expected:?}",
                            id.index()
                        ),
                    ));
                }
            }
            pending.extend(expression_children(expression));
        }
        Ok(())
    }

    fn available_call_protocols(
        &self,
        callee_type: TypeId,
        signature: TypeId,
        assumptions: &BTreeSet<TraitQuery>,
        capabilities: &CapabilityAssumptions,
        analysis: &CapabilityAnalysis,
        context: &str,
    ) -> Result<super::HirClosureProtocols, HirInvariantError> {
        match self
            .program
            .interner
            .kind(callee_type)
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
        {
            TypeKind::Function(_) => {
                return Ok(super::HirClosureProtocols::new(
                    callee_type == signature,
                    callee_type == signature,
                    callee_type == signature,
                ));
            }
            TypeKind::Generated {
                identity,
                arguments,
            } => {
                if let Some(closure) = self.program.closure_by_identity(identity) {
                    let mut interner = self.program.interner.clone();
                    let actual = TypeSubstitution::new(arguments.clone())
                        .apply(&mut interner, closure.function_type)
                        .map_err(|error| HirInvariantError::new(context, error.to_string()))?;
                    return Ok(if actual == signature {
                        closure.protocols
                    } else {
                        super::HirClosureProtocols::new(false, false, false)
                    });
                }
            }
            _ => {}
        }

        let queries = match self
            .program
            .interner
            .kind(callee_type)
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
        {
            TypeKind::GenericParameter(_) => assumptions
                .iter()
                .filter(|query| query.target() == callee_type)
                .cloned()
                .collect::<Vec<_>>(),
            TypeKind::OpaqueResult {
                identity,
                arguments,
            } => {
                let opaque = self.program.opaque_result(identity).ok_or_else(|| {
                    HirInvariantError::new(context, "opaque call target has no contract")
                })?;
                let mut interner = self.program.interner.clone();
                let substitution = TypeSubstitution::new(arguments.clone());
                opaque
                    .bounds
                    .iter()
                    .map(|bound| {
                        let arguments = bound
                            .arguments
                            .iter()
                            .map(|argument| {
                                substitution
                                    .apply(&mut interner, *argument)
                                    .map_err(|error| {
                                        HirInvariantError::new(context, error.to_string())
                                    })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(TraitQuery::from_parts(
                            bound.constructor.clone(),
                            arguments,
                            callee_type,
                        ))
                    })
                    .collect::<Result<Vec<_>, HirInvariantError>>()?
            }
            _ => Vec::new(),
        };
        let mut call = false;
        let mut call_mut = false;
        let mut call_once = false;
        for query in queries {
            if query.arguments() != [signature] {
                continue;
            }
            let HirTraitConstructor::Prelude(name) = query.constructor() else {
                continue;
            };
            match name.as_str() {
                "Call" => call = true,
                "CallMut" => call_mut = true,
                "CallOnce" => call_once = true,
                _ => {}
            }
        }
        call_mut |= call;
        let discard = analysis
            .status(
                self.program,
                callee_type,
                HirCapability::Discard,
                capabilities,
            )
            .map_err(|error| HirInvariantError::new(context, error.to_string()))?
            == super::HirCapabilityStatus::Satisfied;
        call_once |= discard && call_mut;
        Ok(super::HirClosureProtocols::new(call, call_mut, call_once))
    }

    fn call_mut_place_is_available(
        &self,
        expression: HirExpressionId,
        exclusive_parameters: &BTreeSet<crate::resolve::LocalId>,
        mutable_receiver: bool,
        context: &str,
    ) -> Result<bool, HirInvariantError> {
        Ok(match &self.expression(expression, context)?.kind {
            HirExpressionKind::Local(local) => {
                self.local_is_mutable_binding(*local) || exclusive_parameters.contains(local)
            }
            HirExpressionKind::Field { base, .. }
            | HirExpressionKind::TupleField { base, .. }
            | HirExpressionKind::Index { base, .. }
            | HirExpressionKind::Slice { base, .. } => self.call_mut_place_is_available(
                *base,
                exclusive_parameters,
                mutable_receiver,
                context,
            )?,
            HirExpressionKind::Receiver => mutable_receiver,
            _ => false,
        })
    }

    fn verify_type(&self, ty: TypeId, context: impl Into<String>) -> Result<(), HirInvariantError> {
        let context = context.into();
        self.program
            .interner
            .canonical(ty)
            .map(|_| ())
            .map_err(|error| {
                HirInvariantError::new(context, format!("type {ty} is not canonical: {error}"))
            })
    }

    fn verify_symbol(
        &self,
        symbol: crate::resolve::SymbolId,
        expected: &[SymbolKind],
        context: &str,
    ) -> Result<(), HirInvariantError> {
        let declaration = self.resolved.symbol(symbol).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("references unknown symbol#{}", symbol.index()),
            )
        })?;
        if !expected.is_empty() && !expected.contains(&declaration.kind()) {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "symbol#{} has kind {:?}, expected one of {:?}",
                    symbol.index(),
                    declaration.kind(),
                    expected
                ),
            ));
        }
        Ok(())
    }

    fn verify_member(
        &self,
        member: crate::resolve::MemberId,
        expected: &[MemberKind],
        context: &str,
    ) -> Result<&crate::resolve::Member, HirInvariantError> {
        let declaration = self.resolved.member(member).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("references unknown member#{}", member.index()),
            )
        })?;
        if !expected.is_empty() && !expected.contains(&declaration.kind()) {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "member#{} has kind {:?}, expected one of {:?}",
                    member.index(),
                    declaration.kind(),
                    expected
                ),
            ));
        }
        Ok(declaration)
    }

    fn verify_local(
        &self,
        local: crate::resolve::LocalId,
        context: &str,
    ) -> Result<&crate::resolve::LocalBinding, HirInvariantError> {
        self.resolved.local(local).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("references unknown local#{}", local.index()),
            )
        })
    }

    fn verify_resolved_callable_id(
        &self,
        callable: HirCallableId,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        match callable {
            HirCallableId::Symbol(symbol) => {
                self.verify_symbol(symbol, &[SymbolKind::Function], context)
            }
            HirCallableId::Member(member) => {
                let declaration = self.resolved.member(member).ok_or_else(|| {
                    HirInvariantError::new(
                        context,
                        format!("references unknown callable member#{}", member.index()),
                    )
                })?;
                if !declaration.kind().is_callable() {
                    return Err(HirInvariantError::new(
                        context,
                        format!("member#{} is not callable", member.index()),
                    ));
                }
                Ok(())
            }
            HirCallableId::Implementation(method) => self
                .program
                .implementation_method(method)
                .map(|_| ())
                .ok_or_else(|| {
                    HirInvariantError::new(
                        context,
                        format!(
                            "references unknown implementation#{}.method#{}",
                            method.implementation().index(),
                            method.index()
                        ),
                    )
                }),
        }
    }

    fn verify_callable_id(
        &self,
        callable: HirCallableId,
        context: &str,
    ) -> Result<&super::HirCallableSignature, HirInvariantError> {
        self.verify_resolved_callable_id(callable, context)?;
        self.program.callable(callable).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("{} has no HIR signature", callable_context(callable)),
            )
        })
    }

    fn expression(
        &self,
        id: HirExpressionId,
        context: &str,
    ) -> Result<&HirExpression, HirInvariantError> {
        self.program.expression(id).ok_or_else(|| {
            HirInvariantError::new(
                context,
                format!("references unknown expression#{}", id.index()),
            )
        })
    }

    fn expression_before(
        &self,
        child: HirExpressionId,
        parent: HirExpressionId,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        self.expression(child, context)?;
        if child >= parent {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "child expression#{} is not earlier than its parent expression#{}",
                    child.index(),
                    parent.index()
                ),
            ));
        }
        Ok(())
    }

    fn pattern_before(
        &self,
        child: HirPatternId,
        parent: HirPatternId,
        context: &str,
    ) -> Result<(), HirInvariantError> {
        if child.0 as usize >= self.program.patterns.len() {
            return Err(HirInvariantError::new(
                context,
                format!("references unknown pattern#{}", child.index()),
            ));
        }
        if child >= parent {
            return Err(HirInvariantError::new(
                context,
                format!(
                    "child pattern#{} is not earlier than its parent pattern#{}",
                    child.index(),
                    parent.index()
                ),
            ));
        }
        Ok(())
    }
}

fn expression_children(expression: &HirExpression) -> Vec<HirExpressionId> {
    let mut children = Vec::new();
    match &expression.kind {
        HirExpressionKind::Recovery
        | HirExpressionKind::Literal(_)
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Constant(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::SpecializedFunction { .. }
        | HirExpressionKind::PreludeTraitFunction { .. }
        | HirExpressionKind::Closure(_)
        | HirExpressionKind::Receiver
        | HirExpressionKind::Break { .. }
        | HirExpressionKind::Continue { .. } => {}
        HirExpressionKind::InterpolatedString { values, .. }
        | HirExpressionKind::Tuple(values)
        | HirExpressionKind::Array(values)
        | HirExpressionKind::Set(values) => children.extend(values),
        HirExpressionKind::Map { entries, .. } => {
            for entry in entries {
                children.push(entry.key);
                children.push(entry.value);
            }
        }
        HirExpressionKind::Newtype { value, .. }
        | HirExpressionKind::NumericConversion { value, .. }
        | HirExpressionKind::OptionSome { value }
        | HirExpressionKind::ResultOk { value }
        | HirExpressionKind::PropagateOption { value }
        | HirExpressionKind::PropagateResult { value, .. }
        | HirExpressionKind::Coerce { value, .. } => children.push(*value),
        HirExpressionKind::ResultErr { error } | HirExpressionKind::Fail { error } => {
            children.push(*error);
        }
        HirExpressionKind::Record { fields, .. } => {
            children.extend(fields.iter().map(|field| field.value));
        }
        HirExpressionKind::Variant { payload, .. } => match payload {
            HirVariantValue::Unit => {}
            HirVariantValue::Tuple(values) => children.extend(values),
            HirVariantValue::Record(fields) => {
                children.extend(fields.iter().map(|field| field.value));
            }
        },
        HirExpressionKind::RecordUpdate { base, fields } => {
            children.push(*base);
            children.extend(fields.iter().map(|field| field.value));
        }
        HirExpressionKind::Block { statements, tail } => {
            for statement in statements {
                statement_children(statement, &mut children);
            }
            children.extend(tail);
        }
        HirExpressionKind::Prefix { operand, .. }
        | HirExpressionKind::Field { base: operand, .. }
        | HirExpressionKind::TupleField { base: operand, .. } => children.push(*operand),
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
        } => {
            children.push(*left);
            children.push(*right);
        }
        HirExpressionKind::Index { base, index, .. } => {
            children.push(*base);
            children.push(*index);
        }
        HirExpressionKind::Slice {
            base,
            start,
            end,
            step,
        } => {
            children.push(*base);
            children.extend(start);
            children.extend(end);
            children.extend(step);
        }
        HirExpressionKind::Call {
            callee, arguments, ..
        } => {
            children.push(*callee);
            children.extend(arguments.iter().map(|argument| argument.value));
        }
        HirExpressionKind::PreludePanic { message } => children.push(*message),
        HirExpressionKind::PreludeAssert {
            condition,
            message_parts,
            ..
        } => {
            children.push(*condition);
            children.extend(message_parts.iter().map(|part| part.value()));
        }
        HirExpressionKind::BootstrapHostCall { arguments, .. } => children.extend(arguments),
        HirExpressionKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            children.push(*condition);
            children.push(*then_branch);
            children.extend(else_branch);
        }
        HirExpressionKind::Match {
            scrutinee, arms, ..
        } => {
            children.push(*scrutinee);
            for arm in arms {
                children.extend(&arm.guard);
                children.push(arm.body);
            }
        }
        HirExpressionKind::Return { value } => children.extend(value),
    }
    children
}

fn statement_children(statement: &HirStatement, children: &mut Vec<HirExpressionId>) {
    match statement {
        HirStatement::Binding { value, .. }
        | HirStatement::Expression { value, .. }
        | HirStatement::Discard { value, .. } => children.push(*value),
        HirStatement::Assignment { target, value, .. } => {
            assignment_target_children(target, children);
            children.push(*value);
        }
        HirStatement::For { kind, body, .. } => {
            match kind {
                HirForKind::Infinite => {}
                HirForKind::Conditional { condition } => children.push(*condition),
                HirForKind::Iterate { source, .. } => children.push(*source),
            }
            children.push(*body);
        }
    }
}

fn assignment_target_children(root: &HirAssignmentTarget, children: &mut Vec<HirExpressionId>) {
    let mut pending = vec![root];
    while let Some(target) = pending.pop() {
        match &target.kind {
            HirAssignmentTargetKind::Place { place, .. } => children.push(*place),
            HirAssignmentTargetKind::Discard => {}
            HirAssignmentTargetKind::Tuple(items) => pending.extend(items),
        }
    }
}

fn pattern_children(pattern: &HirPattern) -> Vec<HirPatternId> {
    match &pattern.kind {
        HirPatternKind::Tuple(items) => items.clone(),
        HirPatternKind::OptionSome(item)
        | HirPatternKind::ResultOk(item)
        | HirPatternKind::ResultErr(item)
        | HirPatternKind::Newtype { value: item, .. }
        | HirPatternKind::UnionMember { pattern: item, .. } => vec![*item],
        HirPatternKind::Variant { fields, .. } => fields.clone(),
        HirPatternKind::Record { fields, .. } => fields.iter().map(|field| field.pattern).collect(),
        HirPatternKind::Array { prefix, rest } => {
            let mut children = prefix.clone();
            children.extend(rest);
            children
        }
        HirPatternKind::Recovery
        | HirPatternKind::Wildcard
        | HirPatternKind::Binding(_)
        | HirPatternKind::BorrowBinding(_)
        | HirPatternKind::Literal(_)
        | HirPatternKind::OptionNone => Vec::new(),
    }
}

fn category_name(category: HirValueCategory) -> &'static str {
    match category {
        HirValueCategory::Value => "value",
        HirValueCategory::Place => "place",
    }
}

fn generic_bound_type_roots(parameter: &HirGenericParameter) -> Vec<TypeId> {
    parameter
        .bounds
        .iter()
        .flat_map(|bound| bound.arguments.iter().copied())
        .collect()
}

fn nominal_type_roots(shape: &super::HirNominalShape) -> Vec<TypeId> {
    match shape {
        super::HirNominalShape::Newtype { underlying } => vec![*underlying],
        super::HirNominalShape::Record { fields } => fields.iter().map(|field| field.ty).collect(),
        super::HirNominalShape::Enum { variants } => variants
            .iter()
            .flat_map(|variant| match &variant.payload {
                HirVariantPayload::Unit => Vec::new(),
                HirVariantPayload::Tuple(items) => items.clone(),
                HirVariantPayload::Record(fields) => fields.iter().map(|field| field.ty).collect(),
            })
            .collect(),
    }
}

fn same_generic_parameter(left: &HirGenericParameter, right: &HirGenericParameter) -> bool {
    left.local == right.local
        && left.position == right.position
        && left.bounds.len() == right.bounds.len()
        && left.bounds.iter().zip(&right.bounds).all(|(left, right)| {
            left.constructor == right.constructor && left.arguments == right.arguments
        })
}

fn same_generic_bound_groups(
    left: &[Vec<super::HirTraitReference>],
    right: &[Vec<super::HirTraitReference>],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.len() == right.len()
                && left.iter().all(|left| {
                    right.iter().any(|right| {
                        left.constructor == right.constructor && left.arguments == right.arguments
                    })
                })
        })
}

fn identity_belongs_to(module: &ModuleId, identity: &SymbolIdentity) -> bool {
    identity.package() == module.package() && identity.module() == module.path()
}

fn callable_context(callable: HirCallableId) -> String {
    match callable {
        HirCallableId::Symbol(symbol) => format!("callable symbol#{}", symbol.index()),
        HirCallableId::Member(member) => format!("callable member#{}", member.index()),
        HirCallableId::Implementation(method) => format!(
            "callable implementation#{}.method#{}",
            method.implementation().index(),
            method.index()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::hir::{
        ExpressionCheckLimits, HirExpressionKind, HirMatchMode, HirPrefixOperator,
        TypeLoweringLimits, check_expressions, lower_types,
    };
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn checked_program() -> (ResolvedProgram, HirProgram) {
        checked_program_from(
            "fn main() {\n    let value = 1\n    assert(value == 1)\n    _ = value\n}\n",
        )
    }

    fn checked_program_from(source: &str) -> (ResolvedProgram, HirProgram) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:hir-verifier").unwrap(),
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
        let (program, diagnostics) = lower_types(
            &packages,
            &sources,
            [(file, &parsed)],
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 10_000,
                max_trait_obligations: 10_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty());
        let (program, diagnostics, complete) = check_expressions(
            &sources,
            [(file, &parsed)],
            &resolved,
            program,
            ExpressionCheckLimits {
                max_nodes: 10_000,
                max_pattern_steps: 10_000,
                max_trait_obligations: 10_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty());
        assert!(complete);
        (resolved, program)
    }

    #[test]
    fn complete_checked_hir_satisfies_the_mir_entry_contract() {
        let (resolved, program) = checked_program();
        verify_typed_hir(&resolved, &program).unwrap();
    }

    #[test]
    fn availability_and_var_reinitialization_are_reproved_before_mir() {
        let (resolved, mut program) = checked_program_from(
            "fn valid[T: Discard](value: T, other: T, flag: Bool) {\n\
                 if flag {\n\
                     _ = value\n\
                 }\n\
                 _ = other\n\
             }\n",
        );
        let local = |name: &str| {
            resolved
                .locals()
                .find(|local| local.name().as_str() == name)
                .unwrap()
                .id()
        };
        let value = local("value");
        let other = local("other");
        let expression = program
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(expression.kind, HirExpressionKind::Local(local) if local == other)
            })
            .unwrap();
        expression.kind = HirExpressionKind::Local(value);

        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "ownership availability");
        assert!(error.message().contains("after its value moved"));

        let (resolved, mut program) = checked_program_from(
            "fn valid[T: Discard](first: T, second: T): T {\n\
                 var value = first\n\
                 _ = value\n\
                 value = second\n\
                 value\n\
             }\n",
        );
        let mutable = program
            .expressions
            .iter_mut()
            .find_map(|expression| match &mut expression.kind {
                HirExpressionKind::Block { statements, .. } => {
                    statements.iter_mut().find_map(|statement| match statement {
                        HirStatement::Binding { mutable, .. } if *mutable => Some(mutable),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("the fixture contains one var binding");
        *mutable = false;

        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "ownership availability");
        assert!(error.message().contains("after its value moved"));
    }

    #[test]
    fn match_ownership_mode_is_reproved_before_mir() {
        let (resolved, mut program) = checked_program_from(
            "fn consume[T](value: T): T {\n\
                 match value {\n\
                     item => item\n\
                 }\n\
             }\n",
        );
        let mode = program
            .expressions
            .iter_mut()
            .find_map(|expression| match &mut expression.kind {
                HirExpressionKind::Match { mode, .. } => Some(mode),
                _ => None,
            })
            .expect("the fixture contains one match");
        assert_eq!(*mode, HirMatchMode::Consume);
        *mode = HirMatchMode::Observe;

        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "ownership availability");
        assert!(error.message().contains("ownership mode"), "{error}");
    }

    #[test]
    fn opaque_contracts_and_seals_are_reproved_before_mir() {
        const SOURCE: &str = "trait Visible {}\n\
             trait Hidden {}\n\
             type Item = { marker: Unit }\n\
             impl Visible for Item {}\n\
             fn reveal(): impl Visible + Discard { Item { marker: () } }\n";

        let trait_symbol = |resolved: &ResolvedProgram, name: &str| {
            resolved
                .symbols()
                .find(|symbol| symbol.name().as_str() == name)
                .unwrap()
                .id()
        };

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut wrong_bound) = checked_program_from(SOURCE);
        let visible = trait_symbol(&resolved, "Visible");
        let hidden = trait_symbol(&resolved, "Hidden");
        let opaque = wrong_bound
            .callables
            .iter_mut()
            .find_map(|callable| callable.opaque_result.as_mut())
            .unwrap();
        opaque
            .bounds
            .iter_mut()
            .find(|bound| bound.constructor == HirTraitConstructor::Symbol(visible))
            .unwrap()
            .constructor = HirTraitConstructor::Symbol(hidden);
        let error = verify_typed_hir(&resolved, &wrong_bound).unwrap_err();
        assert!(error.message().contains("published bound"));

        let (resolved, mut duplicate_bound) = checked_program_from(SOURCE);
        let opaque = duplicate_bound
            .callables
            .iter_mut()
            .find_map(|callable| callable.opaque_result.as_mut())
            .unwrap();
        opaque.bounds.push(opaque.bounds[0].clone());
        let error = verify_typed_hir(&resolved, &duplicate_bound).unwrap_err();
        assert!(error.message().contains("duplicate normalized contract"));

        let (resolved, mut wrong_seal) = checked_program_from(SOURCE);
        let expression = wrong_seal
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::Coerce {
                        kind: crate::types::Assignability::Opaque,
                        ..
                    }
                )
            })
            .unwrap();
        let HirExpressionKind::Coerce { kind, .. } = &mut expression.kind else {
            unreachable!()
        };
        *kind = crate::types::Assignability::OptionLift;
        let error = verify_typed_hir(&resolved, &wrong_seal).unwrap_err();
        assert!(error.message().contains("closed semantic relation"));
    }

    #[test]
    fn trait_contract_metadata_is_verified_before_mir() {
        const SOURCE: &str = "trait Contract[T: Discard] {\n\
             async fn send(self)\n\
             fn required(self): T\n\
             fn defaulted[U](self, value: U): U { value }\n\
         }\n\
         fn main() {}\n";

        fn trait_owner(resolved: &ResolvedProgram) -> crate::resolve::SymbolId {
            resolved
                .symbols()
                .find(|symbol| symbol.name().as_str() == "Contract")
                .unwrap()
                .id()
        }

        fn method(
            resolved: &ResolvedProgram,
            owner: crate::resolve::SymbolId,
            name: &str,
        ) -> crate::resolve::MemberId {
            resolved
                .members()
                .find(|member| {
                    member.owner() == MemberOwner::Type(owner) && member.name().as_str() == name
                })
                .unwrap()
                .id()
        }

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut missing) = checked_program_from(SOURCE);
        let owner = trait_owner(&resolved);
        let HirTypeDeclarationKind::Trait(definition) =
            &mut missing.declarations.get_mut(&owner).unwrap().kind
        else {
            panic!("Contract must remain a trait")
        };
        definition.methods.pop();
        let error = verify_typed_hir(&resolved, &missing).unwrap_err();
        assert!(error.message().contains("method table"));

        let (resolved, mut wrong_default) = checked_program_from(SOURCE);
        let owner = trait_owner(&resolved);
        let defaulted = method(&resolved, owner, "defaulted");
        let HirTypeDeclarationKind::Trait(definition) =
            &mut wrong_default.declarations.get_mut(&owner).unwrap().kind
        else {
            panic!("Contract must remain a trait")
        };
        definition
            .methods
            .iter_mut()
            .find(|entry| entry.member == defaulted)
            .unwrap()
            .has_default = false;
        let error = verify_typed_hir(&resolved, &wrong_default).unwrap_err();
        assert!(error.message().contains("default-body flag"));

        let (resolved, mut wrong_send) = checked_program_from(SOURCE);
        let owner = trait_owner(&resolved);
        let send = method(&resolved, owner, "send");
        let HirTypeDeclarationKind::Trait(definition) =
            &mut wrong_send.declarations.get_mut(&owner).unwrap().kind
        else {
            panic!("Contract must remain a trait")
        };
        definition
            .methods
            .iter_mut()
            .find(|entry| entry.member == send)
            .unwrap()
            .requires_self_send = false;
        let error = verify_typed_hir(&resolved, &wrong_send).unwrap_err();
        assert!(error.message().contains("Self: Send"));

        let (resolved, mut wrong_arity) = checked_program_from(SOURCE);
        let owner = trait_owner(&resolved);
        let defaulted = method(&resolved, owner, "defaulted");
        wrong_arity
            .callables
            .iter_mut()
            .find(|callable| callable.id == HirCallableId::Member(defaulted))
            .unwrap()
            .generic_arity = 2;
        let error = verify_typed_hir(&resolved, &wrong_arity).unwrap_err();
        assert!(error.message().contains("generic arity"));
    }

    #[test]
    fn implementation_contract_metadata_is_verified_before_mir() {
        const SOURCE: &str = "trait Contract {\n\
             fn required(self): Int\n\
             fn defaulted(self): Bool { true }\n\
         }\n\
         type Item = Int\n\
         impl Contract for Item {\n\
             fn required(self): Int { 1 }\n\
         }\n\
         fn main() {}\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut incomplete) = checked_program_from(SOURCE);
        incomplete.implementations[0].contract_complete = false;
        let error = verify_typed_hir(&resolved, &incomplete).unwrap_err();
        assert!(error.message().contains("contract is incomplete"));

        let (resolved, mut wrong_id) = checked_program_from(SOURCE);
        wrong_id.implementations[0].methods[0].id.index = 1;
        let error = verify_typed_hir(&resolved, &wrong_id).unwrap_err();
        assert!(error.message().contains("table position"));

        let (resolved, mut wrong_signature) = checked_program_from(SOURCE);
        let wrong = wrong_signature.interner.scalar(ScalarType::Unit);
        let method_id = wrong_signature.implementations[0].methods[0].id;
        wrong_signature.implementations[0].methods[0]
            .contract
            .as_mut()
            .unwrap()
            .function_type = wrong;
        wrong_signature
            .callables
            .iter_mut()
            .find(|callable| callable.id == HirCallableId::Implementation(method_id))
            .unwrap()
            .function_type = wrong;
        let error = verify_typed_hir(&resolved, &wrong_signature).unwrap_err();
        assert!(error.message().contains("not derived"));

        let (resolved, mut wrong_key) = checked_program_from(SOURCE);
        wrong_key.implementations[0].methods[0]
            .contract
            .as_mut()
            .unwrap()
            .method = HirTraitMethodKey::Prelude(crate::hir::HirPreludeTraitMethod::Display);
        let error = verify_typed_hir(&resolved, &wrong_key).unwrap_err();
        assert!(error.message().contains("method key kind"));

        let (resolved, mut missing) = checked_program_from(SOURCE);
        missing.implementations[0].methods.clear();
        let error = verify_typed_hir(&resolved, &missing).unwrap_err();
        assert!(
            error.message().contains("required trait method")
                || error.message().contains("one-to-one correspondence")
        );
    }

    #[test]
    fn prelude_trait_function_specializations_are_verified_before_mir() {
        const SOURCE: &str = "type Label = { text: String }\n\
             impl Display for Label {\n\
                 fn display(self): String { self.text }\n\
             }\n\
             fn render(value: Label): String { Display.display(value) }\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut wrong_arity) = checked_program_from(SOURCE);
        let unit = wrong_arity.interner.scalar(ScalarType::Unit);
        let expression = wrong_arity
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::PreludeTraitFunction {
                        ref arguments,
                        ..
                    } if !arguments.is_empty()
                )
            })
            .expect("qualified Display call has a specialized prelude callee");
        let HirExpressionKind::PreludeTraitFunction { arguments, .. } = &mut expression.kind else {
            unreachable!()
        };
        arguments.push(unit);
        let error = verify_typed_hir(&resolved, &wrong_arity).unwrap_err();
        assert!(error.message().contains("specialization"));

        let (resolved, mut wrong_type) = checked_program_from(SOURCE);
        let unit = wrong_type.interner.scalar(ScalarType::Unit);
        let expression = wrong_type
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::PreludeTraitFunction {
                        ref arguments,
                        ..
                    } if !arguments.is_empty()
                )
            })
            .expect("qualified Display call has a specialized prelude callee");
        expression.ty = unit;
        let error = verify_typed_hir(&resolved, &wrong_type).unwrap_err();
        assert!(error.message().contains("closed contract"));
    }

    #[test]
    fn named_function_values_are_closed_and_exact_before_mir() {
        const SOURCE: &str = "fn identity[T](value: T): T { value }\n\
             fn handler(): fn(Int): Int { identity[Int] }\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut open) = checked_program_from(SOURCE);
        let expression = open
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::SpecializedFunction { .. }
                )
            })
            .expect("the generic function value is explicitly specialized");
        let HirExpressionKind::SpecializedFunction { callable, .. } = expression.kind else {
            unreachable!()
        };
        expression.kind = HirExpressionKind::Function(callable);
        let error = verify_typed_hir(&resolved, &open).unwrap_err();
        assert!(
            error
                .message()
                .contains("escaped without one complete specialization")
        );

        let (resolved, mut inexact) = checked_program_from(SOURCE);
        let string = inexact.interner.scalar(ScalarType::String);
        let expression = inexact
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::SpecializedFunction { .. }
                )
            })
            .expect("the generic function value is explicitly specialized");
        let HirExpressionKind::SpecializedFunction { arguments, .. } = &mut expression.kind else {
            unreachable!()
        };
        arguments[0] = string;
        let error = verify_typed_hir(&resolved, &inexact).unwrap_err();
        assert!(error.message().contains("exact substituted signature"));
    }

    #[test]
    fn closure_capture_metadata_is_reproved_before_mir() {
        const SOURCE: &str = "fn build(task: Join[Int, Int]) {\n\
             let offset = 2\n\
             var count = 0\n\
             let closure = (value: Int): Int {\n\
                 count += 1\n\
                 value + offset\n\
             }\n\
             _ = closure\n\
         }\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut wrong_mutability) = checked_program_from(SOURCE);
        let capture = wrong_mutability.closures[0]
            .captures
            .iter_mut()
            .find(|capture| capture.mutable)
            .expect("the closure captures `count` as mutable");
        capture.mutable = false;
        let error = verify_typed_hir(&resolved, &wrong_mutability).unwrap_err();
        assert!(error.message().contains("mutability differs"));

        let (resolved, mut wrong_type) = checked_program_from(SOURCE);
        let boolean = wrong_type.interner.scalar(ScalarType::Bool);
        wrong_type.closures[0].captures[0].ty = boolean;
        let error = verify_typed_hir(&resolved, &wrong_type).unwrap_err();
        assert!(error.message().contains("type differs"));

        let (resolved, mut wrong_construction) = checked_program_from(SOURCE);
        let parameter_span = wrong_construction.closures[0].parameters[0].span;
        wrong_construction
            .expressions
            .iter_mut()
            .find(|expression| matches!(expression.kind, HirExpressionKind::Closure(_)))
            .unwrap()
            .span = parameter_span;
        let error = verify_typed_hir(&resolved, &wrong_construction).unwrap_err();
        assert!(error.message().contains("not one-to-one"));

        let (resolved, mut wrong_protocols) = checked_program_from(SOURCE);
        wrong_protocols.closures[0].protocols =
            crate::hir::HirClosureProtocols::new(true, true, true);
        let error = verify_typed_hir(&resolved, &wrong_protocols).unwrap_err();
        assert!(error.message().contains("protocols"));

        let (resolved, mut wrong_arity) = checked_program_from(SOURCE);
        wrong_arity.closures[0].generic_arity = 1;
        let error = verify_typed_hir(&resolved, &wrong_arity).unwrap_err();
        assert!(error.message().contains("generic arity"));

        let (resolved, mut non_copy_capture) = checked_program_from(SOURCE);
        let join = non_copy_capture
            .interner
            .ids()
            .find(|ty| {
                matches!(
                    non_copy_capture.interner.kind(*ty),
                    Ok(TypeKind::Intrinsic {
                        constructor: IntrinsicType::Join,
                        ..
                    })
                )
            })
            .expect("the source interns its Join parameter type");
        let closure_type = non_copy_capture.closures[0].ty;
        let capture_local = non_copy_capture.closures[0].captures[0].local;
        non_copy_capture.closures[0].captures[0].ty = join;
        non_copy_capture.local_types.insert(capture_local, join);
        non_copy_capture.capability_statuses[closure_type.index() as usize] =
            [HirCapabilityStatus::Unsatisfied; HirCapability::COUNT];
        let error = verify_typed_hir(&resolved, &non_copy_capture).unwrap_err();
        assert!(error.message().contains("M4 closure capture"), "{error}");
        assert!(error.message().contains("Copy"), "{error}");
    }

    #[test]
    fn async_closure_effects_and_protocols_are_reproved_before_mir() {
        const SOURCE: &str = "fn build() {\n\
             let plain = (): Int { 0 }\n\
             _ = plain()\n\
             var count = 0\n\
             let operation = async (): Int {\n\
                 count += 1\n\
                 count\n\
             }\n\
             _ = plain\n\
             _ = operation\n\
         }\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();
        let async_closure = program
            .closures
            .iter()
            .find(|closure| closure.is_async())
            .unwrap();
        assert_eq!(
            async_closure.protocols,
            crate::hir::HirClosureProtocols::new(false, false, true)
        );

        let (resolved, mut wrong_effect) = checked_program_from(SOURCE);
        let sync_type = wrong_effect
            .closures
            .iter()
            .find(|closure| !closure.is_async())
            .unwrap()
            .function_type;
        let async_closure = wrong_effect
            .closures
            .iter_mut()
            .find(|closure| closure.is_async())
            .unwrap();
        async_closure.function_type = sync_type;
        let error = verify_typed_hir(&resolved, &wrong_effect).unwrap_err();
        assert!(error.message().contains("identity kind differs"), "{error}");

        let (resolved, mut wrong_protocols) = checked_program_from(SOURCE);
        wrong_protocols
            .closures
            .iter_mut()
            .find(|closure| closure.is_async())
            .unwrap()
            .protocols = crate::hir::HirClosureProtocols::new(false, true, true);
        let error = verify_typed_hir(&resolved, &wrong_protocols).unwrap_err();
        assert!(error.message().contains("protocols"), "{error}");

        let (resolved, mut effectful_call) = checked_program_from(SOURCE);
        let async_signature = effectful_call
            .closures
            .iter()
            .find(|closure| closure.is_async())
            .unwrap()
            .function_type;
        let signature = effectful_call
            .expressions
            .iter_mut()
            .find_map(|expression| match &mut expression.kind {
                HirExpressionKind::Call { signature, .. } => Some(signature),
                _ => None,
            })
            .unwrap();
        *signature = async_signature;
        let error = verify_typed_hir(&resolved, &effectful_call).unwrap_err();
        assert!(error.message().contains("effectful call"), "{error}");
    }

    #[test]
    fn async_callable_exclusive_parameters_are_rejected_before_mir() {
        const SOURCE: &str = "async fn inspect(value: ref Int) {}\n";
        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut forged) = checked_program_from(SOURCE);
        forged.callables[0].parameters[0].mode = ParameterMode::Mut;
        let error = verify_typed_hir(&resolved, &forged).unwrap_err();
        assert!(error.message().contains("exclusive parameter"), "{error}");
    }

    #[test]
    fn generic_call_protocol_selection_is_reproved_before_mir() {
        const SOURCE: &str = "fn invoke[F: Call[fn(Int): Int]](\n\
             operation: F,\n\
             value: Int,\n\
         ): Int {\n\
             operation(value)\n\
         }\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut wrong_protocol) = checked_program_from(SOURCE);
        let call = wrong_protocol
            .expressions
            .iter_mut()
            .find(|expression| matches!(expression.kind, HirExpressionKind::Call { .. }))
            .expect("the generic body contains one call");
        let HirExpressionKind::Call { protocol, .. } = &mut call.kind else {
            unreachable!()
        };
        *protocol = crate::hir::HirCallProtocol::CallMut;
        let error = verify_typed_hir(&resolved, &wrong_protocol).unwrap_err();
        assert!(error.message().contains("expected Some(Call)"), "{error}");

        let (resolved, mut wrong_signature) = checked_program_from(SOURCE);
        let unit = wrong_signature.interner.scalar(ScalarType::Unit);
        let call = wrong_signature
            .expressions
            .iter_mut()
            .find(|expression| matches!(expression.kind, HirExpressionKind::Call { .. }))
            .expect("the generic body contains one call");
        let HirExpressionKind::Call { signature, .. } = &mut call.kind else {
            unreachable!()
        };
        *signature = unit;
        let error = verify_typed_hir(&resolved, &wrong_signature).unwrap_err();
        assert!(error.message().contains("function signature"), "{error}");

        let (resolved, mut wrong_bound) = checked_program_from(SOURCE);
        let int = wrong_bound.interner.scalar(ScalarType::Int);
        wrong_bound
            .callables
            .iter_mut()
            .find(|callable| !callable.generics.is_empty())
            .unwrap()
            .generics[0]
            .bounds[0]
            .arguments[0] = int;
        let error = verify_typed_hir(&resolved, &wrong_bound).unwrap_err();
        assert!(
            error
                .message()
                .contains("call bounds do not use one exact function signature"),
            "{error}"
        );
    }

    #[test]
    fn iterator_loop_protocols_are_verified_before_mir() {
        const SOURCE: &str = "type Cursor = { value: Int }\n\
             impl Iterator[Int] for Cursor {\n\
                 fn next(mut self): Int? { none }\n\
             }\n\
             fn consume(cursor: Cursor) {\n\
                 for value in cursor {\n\
                     _ = value\n\
                 }\n\
             }\n\
             fn intrinsic(values: Array[Int]) {\n\
                 for value in values {\n\
                     _ = value\n\
                 }\n\
             }\n";
        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut invalid) = checked_program_from(SOURCE);
        let unit = invalid.interner.scalar(ScalarType::Unit);
        let protocol = invalid
            .expressions
            .iter_mut()
            .find_map(|expression| {
                let HirExpressionKind::Block { statements, .. } = &mut expression.kind else {
                    return None;
                };
                statements.iter_mut().find_map(|statement| {
                    let HirStatement::For {
                        kind:
                            HirForKind::Iterate {
                                protocol: HirIterationProtocol::Trait { function_type, .. },
                                ..
                            },
                        ..
                    } = statement
                    else {
                        return None;
                    };
                    Some(function_type)
                })
            })
            .expect("user iterator loop retains a trait protocol");
        *protocol = unit;
        let error = verify_typed_hir(&resolved, &invalid).unwrap_err();
        assert!(error.message().contains("Iterator.next contract"));

        let (resolved, mut wrong_intrinsic_cursor) = checked_program_from(SOURCE);
        let unit = wrong_intrinsic_cursor.interner.scalar(ScalarType::Unit);
        let cursor = wrong_intrinsic_cursor
            .expressions
            .iter_mut()
            .find_map(|expression| {
                let HirExpressionKind::Block { statements, .. } = &mut expression.kind else {
                    return None;
                };
                statements.iter_mut().find_map(|statement| {
                    let HirStatement::For {
                        kind:
                            HirForKind::Iterate {
                                protocol: HirIterationProtocol::Intrinsic { cursor },
                                ..
                            },
                        ..
                    } = statement
                    else {
                        return None;
                    };
                    Some(cursor)
                })
            })
            .expect("the ordinary loop retains its own cursor type");
        *cursor = unit;
        let error = verify_typed_hir(&resolved, &wrong_intrinsic_cursor).unwrap_err();
        assert!(error.message().contains("concrete cursor"), "{error}");
    }

    #[test]
    fn implementation_coherence_is_rederived_before_mir() {
        const SOURCE: &str = "trait Codec[T] {}\n\
             type Json = { id: Int }\n\
             type Xml = { id: Int }\n\
             type Payload = { value: Int }\n\
             impl Codec[Json] for Payload {}\n\
             impl Codec[Xml] for Payload {}\n\
             fn main() {}\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut overlapping) = checked_program_from(SOURCE);
        let first_argument = overlapping.implementations[0].trait_reference.arguments[0];
        overlapping.implementations[1].trait_reference.arguments[0] = first_argument;
        let error = verify_typed_hir(&resolved, &overlapping).unwrap_err();
        assert_eq!(error.context(), "implementation#1");
        assert!(error.message().contains("coherence header overlaps"));
    }

    #[test]
    fn trait_termination_is_rederived_before_mir() {
        const SOURCE: &str = "trait Walk {}\n\
             impl[T: Walk] Walk for Array[T] {}\n\
             fn main() {}\n";

        let (resolved, program) = checked_program_from(SOURCE);
        verify_typed_hir(&resolved, &program).unwrap();

        let (resolved, mut nonterminating) = checked_program_from(SOURCE);
        let parameter = nonterminating.implementations[0].parameters[0].local;
        nonterminating.implementations[0].target = nonterminating.local_types[&parameter];
        let error = verify_typed_hir(&resolved, &nonterminating).unwrap_err();
        assert_eq!(error.context(), "implementation#0");
        assert!(error.message().contains("nonterminating trait cycle"));
        assert!(error.message().contains("[[=]]"));
    }

    #[test]
    fn partial_and_recovery_hir_never_cross_the_mir_boundary() {
        let (resolved, mut program) = checked_program();
        program.expression_check_complete = false;
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "program");
        assert!(error.message().contains("incomplete"));

        let (resolved, mut program) = checked_program();
        program.expressions[0].kind = HirExpressionKind::Recovery;
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "expression#0");
        assert!(error.message().contains("recovery expression"));
    }

    #[test]
    fn every_reachable_hir_type_must_be_canonical() {
        let (resolved, mut program) = checked_program();
        program.expressions[0].ty = program.interner.error();
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "expression#0 type");
        assert!(error.message().contains("not canonical"));
    }

    #[test]
    fn assert_retains_a_nonempty_condition_representation() {
        let (resolved, mut program) = checked_program();
        let expression = program
            .expressions
            .iter_mut()
            .find(|expression| matches!(expression.kind, HirExpressionKind::PreludeAssert { .. }))
            .unwrap();
        let HirExpressionKind::PreludeAssert { condition_repr, .. } = &mut expression.kind else {
            unreachable!()
        };
        assert_eq!(condition_repr, "value == 1");
        condition_repr.clear();
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert!(error.message().contains("prelude assert"));
    }

    #[test]
    fn expression_edges_are_topological_and_metadata_arenas_are_aligned() {
        let (resolved, mut program) = checked_program();
        program.expressions[0].kind = HirExpressionKind::Prefix {
            operator: HirPrefixOperator::Negate,
            operand: HirExpressionId(0),
        };
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "expression#0");
        assert!(error.message().contains("not earlier"));

        let (resolved, mut program) = checked_program();
        program.expression_flows.pop();
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "expression arena");
        assert!(error.message().contains("not aligned"));

        let (resolved, mut program) = checked_program();
        program.capability_statuses.pop();
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "type capabilities");
        assert!(error.message().contains("capability rows"));

        let (resolved, mut program) = checked_program();
        let integer = program.interner.scalar(ScalarType::Int);
        program.capability_statuses[integer.index() as usize][HirCapability::Key.index()] =
            HirCapabilityStatus::Unsatisfied;
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "type capabilities");
        assert!(error.message().contains("Key status"));
    }

    #[test]
    fn capability_consumers_are_reproved_from_typed_hir() {
        let (resolved, mut program) = checked_program_from(
            "fn equal(left: Pointer[Int], right: Pointer[Int]): Bool {\n\
                 _ = left\n\
                 _ = right\n\
                 1 == 1\n\
             }\n",
        );
        let pointers = program
            .expressions
            .iter()
            .enumerate()
            .filter_map(|(index, expression)| {
                matches!(
                    program.interner.kind(expression.ty),
                    Ok(TypeKind::Intrinsic {
                        constructor: IntrinsicType::Pointer,
                        ..
                    })
                )
                .then_some(HirExpressionId(index as u32))
            })
            .collect::<Vec<_>>();
        assert_eq!(pointers.len(), 2);
        let binary = program
            .expressions
            .iter_mut()
            .find(|expression| {
                matches!(
                    expression.kind,
                    HirExpressionKind::Binary {
                        operator: HirBinaryOperator::Equal,
                        ..
                    }
                )
            })
            .unwrap();
        let HirExpressionKind::Binary { left, right, .. } = &mut binary.kind else {
            unreachable!()
        };
        *left = pointers[0];
        *right = pointers[1];

        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert!(error.context().contains("expression#"));
        assert!(error.message().contains("equality requires Equatable"));
    }

    #[test]
    fn local_identity_and_value_category_are_verified() {
        let (resolved, mut program) = checked_program();
        let (index, local) = program
            .expressions
            .iter()
            .enumerate()
            .find_map(|(index, expression)| match expression.kind {
                HirExpressionKind::Local(local) => Some((index, local)),
                _ => None,
            })
            .unwrap();
        program.expressions[index].category = HirValueCategory::Value;
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), format!("expression#{index}"));
        assert!(error.message().contains("expected place"));

        let (resolved, mut program) = checked_program();
        program.local_types.remove(&local);
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert!(error.message().contains("has no checked type"));
    }
}
