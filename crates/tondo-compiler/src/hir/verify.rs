use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::resolve::{LocalKind, MemberKind, ResolvedProgram, SymbolKind};
use crate::types::{IntrinsicType, ScalarType, TypeId, TypeKind};

use super::{
    HirAssignmentTarget, HirAssignmentTargetKind, HirCallableId, HirConstantValue,
    HirConstantValueKind, HirConstantVariantValue, HirExpression, HirExpressionId,
    HirExpressionKind, HirFlow, HirForKind, HirGenericParameter, HirPattern, HirPatternId,
    HirPatternKind, HirProgram, HirStatement, HirTypeDeclarationKind, HirValueCategory,
    HirVariantPayload, HirVariantValue,
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
        if self.program.discard_statuses.len() != self.program.interner.len() {
            return Err(HirInvariantError::new(
                "type capabilities",
                format!(
                    "{} types and {} Discard statuses are not aligned",
                    self.program.interner.len(),
                    self.program.discard_statuses.len()
                ),
            ));
        }

        self.verify_declarations()?;
        self.verify_constants()?;
        self.verify_callables()?;
        self.verify_annotations_and_locals()?;
        self.verify_member_references()?;
        self.verify_patterns()?;
        let loops = self.collect_loops()?;
        self.verify_expressions(&loops)?;
        self.verify_bodies()?;
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
                HirTypeDeclarationKind::Trait => &[SymbolKind::Trait][..],
            };
            self.verify_symbol(declaration.symbol, expected, &context)?;
            self.verify_generics(&declaration.parameters, &context)?;
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
                HirTypeDeclarationKind::Trait => {}
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
            self.verify_generics(&callable.generics, &context)?;
            self.verify_type(callable.outcome, format!("{context} outcome"))?;
            self.verify_type(callable.function_type, format!("{context} function type"))?;
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

    fn verify_generics(
        &self,
        generics: &[HirGenericParameter],
        context: &str,
    ) -> Result<(), HirInvariantError> {
        for (index, generic) in generics.iter().enumerate() {
            if generic.position != index as u32 {
                return Err(HirInvariantError::new(
                    context,
                    format!(
                        "generic local#{} has position {}, expected {index}",
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
            for bound in &generic.bounds {
                if let super::HirTraitConstructor::Symbol(symbol) = bound.constructor {
                    self.verify_symbol(symbol, &[SymbolKind::Trait], context)?;
                }
                for argument in &bound.arguments {
                    self.verify_type(*argument, format!("{context} generic bound"))?;
                }
            }
        }
        Ok(())
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
                if arguments.len() != signature.generics.len() {
                    return Err(HirInvariantError::new(
                        context,
                        format!(
                            "specialization has {} arguments for {} generic parameters",
                            arguments.len(),
                            signature.generics.len()
                        ),
                    ));
                }
                for argument in arguments {
                    self.verify_type(*argument, format!("{context} specialization argument"))?;
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
            HirExpressionKind::Call { callee, arguments } => {
                let callee = self.expression(*callee, context)?;
                if !matches!(
                    self.program.interner.kind(callee.ty),
                    Ok(crate::types::TypeKind::Function(_))
                ) {
                    return Err(HirInvariantError::new(
                        context,
                        "call callee does not have a function type",
                    ));
                }
                for argument in arguments {
                    if argument.target == super::HirCallArgumentTarget::Invalid {
                        return Err(HirInvariantError::new(
                            context,
                            "call retains an invalid argument association",
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
            HirExpressionKind::Recovery
            | HirExpressionKind::Literal(_)
            | HirExpressionKind::InterpolatedString { .. }
            | HirExpressionKind::Receiver
            | HirExpressionKind::Tuple(_)
            | HirExpressionKind::Array(_)
            | HirExpressionKind::Map { .. }
            | HirExpressionKind::Set(_)
            | HirExpressionKind::NumericConversion { .. }
            | HirExpressionKind::Block { .. }
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
            | HirExpressionKind::Continue { .. }
            | HirExpressionKind::Coerce { .. } => {}
        }
        Ok(())
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
    ) -> Result<(), HirInvariantError> {
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
        Ok(())
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
            HirCallableId::Implementation(_) => Ok(()),
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
        HirExpressionKind::Call { callee, arguments } => {
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
        HirExpressionKind::Match { scrutinee, arms } => {
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

fn callable_context(callable: HirCallableId) -> String {
    match callable {
        HirCallableId::Symbol(symbol) => format!("callable symbol#{}", symbol.index()),
        HirCallableId::Member(member) => format!("callable member#{}", member.index()),
        HirCallableId::Implementation(span) => format!(
            "callable implementation in {} at bytes {}..{}",
            span.file(),
            span.range().start(),
            span.range().end()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::hir::{
        ExpressionCheckLimits, HirExpressionKind, HirPrefixOperator, TypeLoweringLimits,
        check_expressions, lower_types,
    };
    use crate::package::PackageGraph;
    use crate::resolve::{ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn checked_program() -> (ResolvedProgram, HirProgram) {
        let source = "fn main() {\n    let value = 1\n    assert(value == 1)\n    _ = value\n}\n";
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
        program.discard_statuses.pop();
        let error = verify_typed_hir(&resolved, &program).unwrap_err();
        assert_eq!(error.context(), "type capabilities");
        assert!(error.message().contains("Discard statuses"));
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
