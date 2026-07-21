//! Immutable semantic snapshot exposed by the compilation driver.

use std::cmp::Ordering;

use crate::hir::{
    HirCallableId, HirCallableSignature, HirExpression, HirExpressionId, HirExpressionKind,
    HirNominalShape, HirProgram, HirVariantPayload,
};
use crate::package::ModuleId;
use crate::resolve::{MemberId, ResolvedEntity, ResolvedName, ResolvedProgram};
use crate::source::{FileId, SourceDatabase, Span, TextRange};
use crate::types::{ScalarType, TypeError, TypeId, TypeInterner, TypeKind};

/// A declaration or reference resolved within one [`SemanticModel`] snapshot.
///
/// Numeric IDs are intentionally snapshot-local. Global declarations expose
/// their stable [`crate::package::SymbolIdentity`] through [`ResolvedProgram`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticEntity {
    Name(ResolvedName),
    Member(MemberId),
    Module(ModuleId),
    ContextualCandidates {
        type_name: ResolvedName,
        value_name: ResolvedName,
    },
}

impl From<&ResolvedEntity> for SemanticEntity {
    fn from(entity: &ResolvedEntity) -> Self {
        match entity {
            ResolvedEntity::Name(name) => Self::Name(name.clone()),
            ResolvedEntity::Module(module) => Self::Module(module.clone()),
            ResolvedEntity::ContextualCandidates {
                type_name,
                value_name,
            } => Self::ContextualCandidates {
                type_name: type_name.clone(),
                value_name: value_name.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticField {
    member: MemberId,
    ty: TypeId,
}

impl SemanticField {
    pub fn member(&self) -> MemberId {
        self.member
    }

    /// Returns a type in the enum declaration's generic binder environment.
    pub fn ty(&self) -> TypeId {
        self.ty
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticVariantPayload {
    Unit,
    Tuple(Vec<TypeId>),
    Record(Vec<SemanticField>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEnumVariant {
    member: MemberId,
    payload: SemanticVariantPayload,
}

impl SemanticEnumVariant {
    pub fn member(&self) -> MemberId {
        self.member
    }

    pub fn payload(&self) -> &SemanticVariantPayload {
        &self.payload
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTypeMembers {
    /// Members are normalized, duplicate-free, and canonically ordered.
    Union(Vec<TypeId>),
    /// Payload types use generic parameter positions; `arguments` supplies the
    /// concrete substitution for the queried nominal instance.
    Enum {
        arguments: Vec<TypeId>,
        variants: Vec<SemanticEnumVariant>,
    },
}

/// A read-only semantic snapshot tied to the exact source database that
/// produced it.
#[derive(Debug)]
pub struct SemanticModel {
    sources: SourceDatabase,
    resolved: ResolvedProgram,
    hir: Option<HirProgram>,
}

impl SemanticModel {
    pub(crate) fn after_resolution(sources: SourceDatabase, resolved: ResolvedProgram) -> Self {
        Self {
            sources,
            resolved,
            hir: None,
        }
    }

    pub(crate) fn with_hir(
        sources: SourceDatabase,
        resolved: ResolvedProgram,
        hir: HirProgram,
    ) -> Self {
        Self {
            sources,
            resolved,
            hir: Some(hir),
        }
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn resolved(&self) -> &ResolvedProgram {
        &self.resolved
    }

    pub fn hir(&self) -> Option<&HirProgram> {
        self.hir.as_ref()
    }

    pub fn interner(&self) -> Option<&TypeInterner> {
        self.hir.as_ref().map(HirProgram::interner)
    }

    /// True only when expression checking covered every construct in the
    /// snapshot. Symbol queries remain useful when this is false.
    pub fn expression_check_complete(&self) -> bool {
        self.hir
            .as_ref()
            .is_some_and(HirProgram::expression_check_complete)
    }

    pub fn canonical_type(&self, ty: TypeId) -> Result<Option<String>, TypeError> {
        self.interner()
            .map(|interner| interner.canonical(ty))
            .transpose()
    }

    pub fn type_annotation_at(&self, file: FileId, range: TextRange) -> Option<TypeId> {
        self.hir.as_ref()?.type_at(file, range)
    }

    pub fn expression_at(
        &self,
        file: FileId,
        range: TextRange,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        let hir = self.hir.as_ref()?;
        hir.expression_at(file, range)
            .or_else(|| hir.expression_covering(file, range))
    }

    pub fn expression_containing(
        &self,
        file: FileId,
        offset: u32,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.hir.as_ref()?.expression_containing(file, offset)
    }

    pub fn expression_type_at(&self, file: FileId, range: TextRange) -> Option<TypeId> {
        self.expression_at(file, range)
            .map(|(_, expression)| expression.ty())
    }

    pub fn expression_type_containing(&self, file: FileId, offset: u32) -> Option<TypeId> {
        self.expression_containing(file, offset)
            .map(|(_, expression)| expression.ty())
    }

    /// Returns every semantic interpretation attached to this exact source
    /// range. Multiple entries are preserved for declarations that inhabit
    /// separate namespaces or shorthand patterns that name both a field and a
    /// local binding.
    pub fn entities_at(&self, file: FileId, range: TextRange) -> Vec<SemanticEntity> {
        let mut entities = Vec::new();
        for (candidate_range, entity) in self.entity_occurrences(file) {
            if candidate_range == range {
                push_unique(&mut entities, entity);
            }
        }
        entities
    }

    /// Uses half-open source ranges and selects the narrowest name occurrence
    /// under the byte offset. Ties retain all distinct semantic entities.
    pub fn entities_containing(&self, file: FileId, offset: u32) -> Vec<SemanticEntity> {
        let mut best_length = None;
        let mut entities = Vec::new();
        for (range, entity) in self.entity_occurrences(file) {
            if !range_contains_offset(range, offset) {
                continue;
            }
            let length = range.end().saturating_sub(range.start());
            match best_length {
                None => {
                    best_length = Some(length);
                    entities.push(entity);
                }
                Some(best) if length < best => {
                    best_length = Some(length);
                    entities.clear();
                    entities.push(entity);
                }
                Some(best) if length == best => push_unique(&mut entities, entity),
                Some(_) => {}
            }
        }
        entities
    }

    /// Returns use-site references only; the declaration span itself is not
    /// included. Results use the deterministic logical-source order.
    pub fn references(&self, entity: &SemanticEntity) -> Vec<Span> {
        let mut spans: Vec<Span> = match entity {
            SemanticEntity::Member(member) => self
                .hir
                .iter()
                .flat_map(|hir| hir.member_references())
                .filter(|reference| reference.member() == *member)
                .map(|reference| reference.span())
                .collect(),
            _ => self
                .resolved
                .references()
                .filter(|reference| resolved_reference_matches(entity, reference.entity()))
                .filter_map(|reference| self.sources.span(reference.file(), reference.range()).ok())
                .collect(),
        };
        spans.sort_by(|left, right| self.compare_spans(*left, *right));
        spans.dedup();
        spans
    }

    pub fn declaration_span(&self, entity: &SemanticEntity) -> Option<Span> {
        match entity {
            SemanticEntity::Name(ResolvedName::Symbol(symbol)) => {
                self.resolved.symbol(*symbol).map(|symbol| symbol.span())
            }
            SemanticEntity::Name(ResolvedName::Local(local)) => {
                self.resolved.local(*local).map(|local| local.span())
            }
            SemanticEntity::Member(member) => {
                self.resolved.member(*member).map(|member| member.span())
            }
            SemanticEntity::Name(
                ResolvedName::Receiver
                | ResolvedName::ContextualSelf
                | ResolvedName::Prelude { .. }
                | ResolvedName::External { .. },
            )
            | SemanticEntity::Module(_)
            | SemanticEntity::ContextualCandidates { .. } => None,
        }
    }

    pub fn signature(&self, entity: &SemanticEntity) -> Option<&HirCallableSignature> {
        let callable = match entity {
            SemanticEntity::Name(ResolvedName::Symbol(symbol)) => HirCallableId::Symbol(*symbol),
            SemanticEntity::Member(member) => HirCallableId::Member(*member),
            _ => return None,
        };
        self.hir.as_ref()?.callable(callable)
    }

    pub fn signature_at(&self, file: FileId, range: TextRange) -> Option<&HirCallableSignature> {
        self.unique_signature(self.entities_at(file, range))
    }

    pub fn signature_containing(&self, file: FileId, offset: u32) -> Option<&HirCallableSignature> {
        self.unique_signature(self.entities_containing(file, offset))
    }

    /// Returns enum variants or normalized union members for the queried type.
    pub fn type_members(&self, ty: TypeId) -> Result<Option<SemanticTypeMembers>, TypeError> {
        let Some(hir) = &self.hir else {
            return Ok(None);
        };
        match hir.interner().kind(ty)? {
            TypeKind::Union(members) => Ok(Some(SemanticTypeMembers::Union(members.clone()))),
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                let Some(symbol) = self
                    .resolved
                    .symbols()
                    .find(|symbol| symbol.identity() == identity)
                    .map(|symbol| symbol.id())
                else {
                    return Ok(None);
                };
                let Some(declaration) = hir.declaration(symbol) else {
                    return Ok(None);
                };
                let crate::hir::HirTypeDeclarationKind::Nominal(definition) = declaration.kind()
                else {
                    return Ok(None);
                };
                let HirNominalShape::Enum { variants } = definition.shape() else {
                    return Ok(None);
                };
                let variants = variants
                    .iter()
                    .map(|variant| SemanticEnumVariant {
                        member: variant.member(),
                        payload: match variant.payload() {
                            HirVariantPayload::Unit => SemanticVariantPayload::Unit,
                            HirVariantPayload::Tuple(types) => {
                                SemanticVariantPayload::Tuple(types.clone())
                            }
                            HirVariantPayload::Record(fields) => SemanticVariantPayload::Record(
                                fields
                                    .iter()
                                    .map(|field| SemanticField {
                                        member: field.member(),
                                        ty: field.ty(),
                                    })
                                    .collect(),
                            ),
                        },
                    })
                    .collect();
                Ok(Some(SemanticTypeMembers::Enum {
                    arguments: arguments.clone(),
                    variants,
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn call_expression_at(
        &self,
        file: FileId,
        range: TextRange,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.hir
            .as_ref()?
            .expressions_with_ids()
            .filter(|(_, expression)| {
                expression.span().file() == file
                    && range_contains_range(expression.span().range(), range)
                    && matches!(expression.kind(), HirExpressionKind::Call { .. })
            })
            .min_by_key(|(id, expression)| {
                (
                    expression
                        .span()
                        .range()
                        .end()
                        .saturating_sub(expression.span().range().start()),
                    std::cmp::Reverse(id.index()),
                )
            })
    }

    pub fn call_expression_containing(
        &self,
        file: FileId,
        offset: u32,
    ) -> Option<(HirExpressionId, &HirExpression)> {
        self.hir
            .as_ref()?
            .expressions_with_ids()
            .filter(|(_, expression)| {
                expression.span().file() == file
                    && range_contains_offset(expression.span().range(), offset)
                    && matches!(expression.kind(), HirExpressionKind::Call { .. })
            })
            .min_by_key(|(id, expression)| {
                (
                    expression
                        .span()
                        .range()
                        .end()
                        .saturating_sub(expression.span().range().start()),
                    std::cmp::Reverse(id.index()),
                )
            })
    }

    pub fn call_signature(&self, call: HirExpressionId) -> Option<&HirCallableSignature> {
        let hir = self.hir.as_ref()?;
        let expression = hir.expression(call)?;
        let HirExpressionKind::Call { callee, .. } = expression.kind() else {
            return None;
        };
        let mut callee = *callee;
        loop {
            match hir.expression(callee)?.kind() {
                HirExpressionKind::Function(callable)
                | HirExpressionKind::SpecializedFunction { callable, .. } => {
                    return hir.callable(*callable);
                }
                HirExpressionKind::Coerce { value, .. } => callee = *value,
                _ => return None,
            }
        }
    }

    /// Returns `None` when `expression` is not a checked call or its recovery
    /// type is unavailable. An empty vector means the call is infallible.
    pub fn closed_call_errors(
        &self,
        expression: HirExpressionId,
    ) -> Result<Option<Vec<TypeId>>, TypeError> {
        let Some(hir) = &self.hir else {
            return Ok(None);
        };
        let Some(expression) = hir.expression(expression) else {
            return Ok(None);
        };
        if !matches!(expression.kind(), HirExpressionKind::Call { .. }) {
            return Ok(None);
        }
        let outcome = expression.ty();
        let error = match hir.interner().kind(outcome)? {
            TypeKind::Error | TypeKind::Inference(_) => return Ok(None),
            TypeKind::Result { error, .. } => Some(*error),
            _ => None,
        };
        let Some(error) = error else {
            return Ok(Some(Vec::new()));
        };
        Ok(Some(match hir.interner().kind(error)? {
            TypeKind::Error | TypeKind::Inference(_) => return Ok(None),
            TypeKind::Scalar(ScalarType::Never) => Vec::new(),
            TypeKind::Union(members) => members.clone(),
            _ => vec![error],
        }))
    }

    pub fn closed_call_errors_at(
        &self,
        file: FileId,
        range: TextRange,
    ) -> Result<Option<Vec<TypeId>>, TypeError> {
        let Some((expression, _)) = self.call_expression_at(file, range) else {
            return Ok(None);
        };
        self.closed_call_errors(expression)
    }

    pub fn closed_call_errors_containing(
        &self,
        file: FileId,
        offset: u32,
    ) -> Result<Option<Vec<TypeId>>, TypeError> {
        let Some((expression, _)) = self.call_expression_containing(file, offset) else {
            return Ok(None);
        };
        self.closed_call_errors(expression)
    }

    fn unique_signature(
        &self,
        entities: impl IntoIterator<Item = SemanticEntity>,
    ) -> Option<&HirCallableSignature> {
        let mut found = None;
        for entity in entities {
            let Some(signature) = self.signature(&entity) else {
                continue;
            };
            if found.is_some_and(|existing: &HirCallableSignature| existing.id() != signature.id())
            {
                return None;
            }
            found = Some(signature);
        }
        found
    }

    fn entity_occurrences(&self, file: FileId) -> Vec<(TextRange, SemanticEntity)> {
        let mut occurrences = Vec::new();
        occurrences.extend(
            self.resolved
                .references()
                .filter(|reference| reference.file() == file)
                .map(|reference| (reference.range(), SemanticEntity::from(reference.entity()))),
        );
        if let Some(hir) = &self.hir {
            occurrences.extend(
                hir.member_references()
                    .filter(|reference| reference.span().file() == file)
                    .map(|reference| {
                        (
                            reference.span().range(),
                            SemanticEntity::Member(reference.member()),
                        )
                    }),
            );
        }
        occurrences.extend(
            self.resolved
                .symbols()
                .filter(|symbol| symbol.span().file() == file)
                .map(|symbol| {
                    (
                        symbol.span().range(),
                        SemanticEntity::Name(ResolvedName::Symbol(symbol.id())),
                    )
                }),
        );
        occurrences.extend(
            self.resolved
                .members()
                .filter(|member| member.span().file() == file)
                .map(|member| (member.span().range(), SemanticEntity::Member(member.id()))),
        );
        occurrences.extend(
            self.resolved
                .locals()
                .filter(|local| local.span().file() == file)
                .map(|local| {
                    (
                        local.span().range(),
                        SemanticEntity::Name(ResolvedName::Local(local.id())),
                    )
                }),
        );
        if let Some(resolution) = self.resolved.file(file) {
            occurrences.extend(resolution.imports().values().map(|import| {
                (
                    import.span().range(),
                    SemanticEntity::Module(import.module().clone()),
                )
            }));
        }
        occurrences
    }

    fn compare_spans(&self, left: Span, right: Span) -> Ordering {
        let left_file = self
            .sources
            .get(left.file())
            .expect("semantic spans retain valid source files");
        let right_file = self
            .sources
            .get(right.file())
            .expect("semantic spans retain valid source files");
        (
            left_file.source_id(),
            left_file.module(),
            left_file.path(),
            left.range().start(),
            left.range().end(),
        )
            .cmp(&(
                right_file.source_id(),
                right_file.module(),
                right_file.path(),
                right.range().start(),
                right.range().end(),
            ))
    }
}

fn resolved_reference_matches(query: &SemanticEntity, actual: &ResolvedEntity) -> bool {
    match query {
        SemanticEntity::Name(expected) => match actual {
            ResolvedEntity::Name(actual) => actual == expected,
            ResolvedEntity::ContextualCandidates {
                type_name,
                value_name,
            } => type_name == expected || value_name == expected,
            ResolvedEntity::Module(_) => false,
        },
        SemanticEntity::Module(expected) => {
            matches!(actual, ResolvedEntity::Module(actual) if actual == expected)
        }
        SemanticEntity::ContextualCandidates {
            type_name: expected_type,
            value_name: expected_value,
        } => matches!(
            actual,
            ResolvedEntity::ContextualCandidates {
                type_name,
                value_name,
            } if type_name == expected_type && value_name == expected_value
        ),
        SemanticEntity::Member(_) => false,
    }
}

fn push_unique(entities: &mut Vec<SemanticEntity>, entity: SemanticEntity) {
    if !entities.contains(&entity) {
        entities.push(entity);
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use super::*;
    use crate::driver::{
        BuildTarget, CompilationOutput, CompilationRequest, CompilationStatus, DiagnosticFormat,
        HostProfile, Operation, ResourceLimits, SourceForm, execute,
    };
    use crate::package::{Edition, PackageGraph};
    use crate::resolve::{MemberKind, SymbolKind};
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};

    fn compile(source: &str) -> CompilationOutput {
        compile_operation(source, Operation::Check)
    }

    fn compile_operation(source: &str, operation: Operation) -> CompilationOutput {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:semantic-test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(source.as_bytes().to_vec()),
            ))
            .unwrap();
        let packages = PackageGraph::loose(&sources, root).unwrap();
        execute(
            CompilationRequest::new(
                operation,
                Edition::V0_1,
                BuildTarget::vm_hosted(),
                HostProfile::Hosted,
                BTreeSet::new(),
                DiagnosticFormat::Json,
                SourceForm::Module,
                ResourceLimits::default(),
                packages,
                sources,
                root,
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn nth_range(source: &str, needle: &str, occurrence: usize) -> TextRange {
        let start = source
            .match_indices(needle)
            .nth(occurrence)
            .map(|(start, _)| start)
            .unwrap_or_else(|| panic!("missing occurrence {occurrence} of {needle:?}"));
        TextRange::new(
            u32::try_from(start).unwrap(),
            u32::try_from(start + needle.len()).unwrap(),
        )
        .unwrap()
    }

    fn range_in(source: &str, anchor: &str, needle: &str) -> TextRange {
        let anchor_start = source
            .find(anchor)
            .unwrap_or_else(|| panic!("missing anchor {anchor:?}"));
        let relative = anchor
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in anchor {anchor:?}"));
        let start = anchor_start + relative;
        TextRange::new(
            u32::try_from(start).unwrap(),
            u32::try_from(start + needle.len()).unwrap(),
        )
        .unwrap()
    }

    fn only_symbol(entities: &[SemanticEntity]) -> crate::resolve::SymbolId {
        let symbols = entities
            .iter()
            .filter_map(|entity| match entity {
                SemanticEntity::Name(ResolvedName::Symbol(symbol)) => Some(*symbol),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(symbols.len(), 1, "unexpected entities: {entities:#?}");
        symbols[0]
    }

    fn only_local(entities: &[SemanticEntity]) -> crate::resolve::LocalId {
        let locals = entities
            .iter()
            .filter_map(|entity| match entity {
                SemanticEntity::Name(ResolvedName::Local(local)) => Some(*local),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(locals.len(), 1, "unexpected entities: {entities:#?}");
        locals[0]
    }

    #[test]
    fn driver_retains_contextual_expression_types_in_a_semantic_snapshot() {
        let source = "fn main() {\n    let maybe: Int? = 42\n}\n";
        let output = compile(source);
        assert_eq!(output.status(), CompilationStatus::Success);
        assert!(output.diagnostics().diagnostics().is_empty());
        let model = output.semantic_model().expect("semantic checking ran");
        assert!(model.expression_check_complete());

        let literal = nth_range(source, "42", 0);
        let ty = model
            .expression_type_at(FileId::from_index(0).unwrap(), literal)
            .unwrap_or_else(|| {
                panic!(
                    "missing {literal:?}; expressions: {:#?}",
                    model.hir().unwrap().expressions().collect::<Vec<_>>()
                )
            });
        assert_eq!(model.canonical_type(ty).unwrap().as_deref(), Some("Int?"));
        assert_eq!(
            model.expression_type_containing(FileId::from_index(0).unwrap(), literal.start()),
            Some(ty)
        );
    }

    #[test]
    fn entities_references_and_signatures_share_the_same_snapshot_ids() {
        let source = "fn add(left: Int, right: Int): Int {\n    left + right\n}\nfn main() {\n    let result = add(1, 2)\n}\n";
        let output = compile(source);
        let model = output.semantic_model().unwrap();
        let file = FileId::from_index(0).unwrap();
        let declaration = nth_range(source, "add", 0);
        let usage = nth_range(source, "add", 1);
        let symbol = only_symbol(&model.entities_at(file, declaration));
        assert_eq!(only_symbol(&model.entities_at(file, usage)), symbol);
        assert_eq!(
            only_symbol(&model.entities_containing(file, usage.start() + 1)),
            symbol
        );
        assert!(model.entities_containing(file, usage.end()).is_empty());

        let entity = SemanticEntity::Name(ResolvedName::Symbol(symbol));
        assert_eq!(
            model
                .references(&entity)
                .iter()
                .map(|span| span.range())
                .collect::<Vec<_>>(),
            [usage]
        );
        assert_eq!(
            model.declaration_span(&entity).unwrap().range(),
            declaration
        );

        let signature = model.signature_at(file, usage).unwrap();
        assert_eq!(signature.id(), HirCallableId::Symbol(symbol));
        assert_eq!(signature.parameters().len(), 2);
        assert_eq!(
            model
                .canonical_type(signature.outcome())
                .unwrap()
                .as_deref(),
            Some("Int")
        );

        let left_declaration = nth_range(source, "left", 0);
        let left_usage = nth_range(source, "left", 1);
        let local = only_local(&model.entities_at(file, left_declaration));
        assert_eq!(only_local(&model.entities_at(file, left_usage)), local);
        assert_eq!(
            model
                .references(&SemanticEntity::Name(ResolvedName::Local(local)))
                .iter()
                .map(|span| span.range())
                .collect::<Vec<_>>(),
            [left_usage]
        );

        let call_range = nth_range(source, "add(1, 2)", 0);
        let (call, _) = model.call_expression_at(file, call_range).unwrap();
        assert_eq!(model.call_signature(call).unwrap().id(), signature.id());
        assert_eq!(model.closed_call_errors(call).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn a_newtype_declaration_preserves_both_language_namespaces() {
        let source = "type UserId = Int\n";
        let output = compile(source);
        let model = output.semantic_model().unwrap();
        let entities = model.entities_at(
            FileId::from_index(0).unwrap(),
            nth_range(source, "UserId", 0),
        );
        let kinds = entities
            .iter()
            .filter_map(|entity| match entity {
                SemanticEntity::Name(ResolvedName::Symbol(symbol)) => {
                    model.resolved().symbol(*symbol).map(|symbol| symbol.kind())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&SymbolKind::Type));
        assert!(kinds.contains(&SymbolKind::NewtypeConstructor));
    }

    #[test]
    fn inherent_method_declarations_expose_receiver_signatures() {
        let source =
            "type Counter = { value: Int }\nfn Counter.read(self): Int {\n    self.value\n}\n";
        let output = compile(source);
        let model = output.semantic_model().unwrap();
        let file = FileId::from_index(0).unwrap();
        let method_range = nth_range(source, "read", 0);
        let member = model
            .entities_at(file, method_range)
            .into_iter()
            .find_map(|entity| match entity {
                SemanticEntity::Member(member) => Some(member),
                _ => None,
            })
            .expect("method declaration resolves to a member");
        assert_eq!(
            model.resolved().member(member).unwrap().kind(),
            MemberKind::InherentMethod
        );
        let signature = model
            .signature(&SemanticEntity::Member(member))
            .expect("inherent methods have HIR signatures");
        assert_eq!(signature.id(), HirCallableId::Member(member));
        assert_eq!(signature.parameters().len(), 1);
        assert!(signature.parameters()[0].is_receiver());
        assert_eq!(
            model
                .canonical_type(signature.outcome())
                .unwrap()
                .as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn enum_union_and_member_queries_are_structural() {
        let source = "type Pair[T] = {\n    first: T\n    second: T\n}\nenum Choice[T] {\n    Empty\n    Item(T)\n    Named { value: T }\n}\nfn read(pair: Pair[Int]): Int {\n    pair.first\n}\nfn inspect(subject: Choice[Int]): Int {\n    match subject {\n        Choice.Empty => 0\n        Choice.Item(number) => number\n        Choice.Named { value } => value\n    }\n}\nfn union(value: Int | String): Int {\n    match value {\n        Int(number) => number\n        String(_) => 0\n    }\n}\n";
        let output = compile(source);
        let model = output.semantic_model().unwrap();
        assert!(model.expression_check_complete());
        let file = FileId::from_index(0).unwrap();

        let subject_usage = range_in(source, "match subject", "subject");
        let choice_type = model.expression_type_at(file, subject_usage).unwrap();
        let Some(SemanticTypeMembers::Enum {
            arguments,
            variants,
        }) = model.type_members(choice_type).unwrap()
        else {
            panic!("Choice[Int] has enum members");
        };
        assert_eq!(arguments.len(), 1);
        assert_eq!(
            model.canonical_type(arguments[0]).unwrap().as_deref(),
            Some("Int")
        );
        assert_eq!(variants.len(), 3);
        let variant_names = variants
            .iter()
            .map(|variant| {
                model
                    .resolved()
                    .member(variant.member())
                    .unwrap()
                    .name()
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(variant_names, ["Empty", "Item", "Named"]);
        assert!(matches!(
            variants[0].payload(),
            SemanticVariantPayload::Unit
        ));
        let SemanticVariantPayload::Tuple(item_payload) = variants[1].payload() else {
            panic!("Item has tuple payload");
        };
        assert_eq!(
            model.canonical_type(item_payload[0]).unwrap().as_deref(),
            Some("$0")
        );
        let SemanticVariantPayload::Record(named_payload) = variants[2].payload() else {
            panic!("Named has record payload");
        };
        assert_eq!(named_payload.len(), 1);
        assert_eq!(
            model
                .canonical_type(named_payload[0].ty())
                .unwrap()
                .as_deref(),
            Some("$0")
        );

        let union_usage = range_in(source, "match value", "value");
        let union_type = model.expression_type_at(file, union_usage).unwrap();
        let Some(SemanticTypeMembers::Union(members)) = model.type_members(union_type).unwrap()
        else {
            panic!("Int | String has union members");
        };
        let member_names = members
            .iter()
            .map(|member| model.canonical_type(*member).unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(member_names, ["Int", "String"]);

        let first_use = nth_range(source, "first", 1);
        let first_member = model
            .entities_at(file, first_use)
            .into_iter()
            .find_map(|entity| match entity {
                SemanticEntity::Member(member) => Some(member),
                _ => None,
            })
            .expect("field access resolves to a member");
        assert_eq!(
            model.resolved().member(first_member).unwrap().kind(),
            MemberKind::RecordField
        );
        assert!(
            model
                .references(&SemanticEntity::Member(first_member))
                .iter()
                .any(|span| span.range() == first_use)
        );

        let empty_use = nth_range(source, "Empty", 1);
        let empty_member = model
            .entities_at(file, empty_use)
            .into_iter()
            .find_map(|entity| match entity {
                SemanticEntity::Member(member) => Some(member),
                _ => None,
            })
            .expect("variant pattern resolves to a member");
        assert_eq!(
            model.resolved().member(empty_member).unwrap().kind(),
            MemberKind::EnumVariant
        );
        assert_eq!(
            model
                .references(&SemanticEntity::Member(empty_member))
                .iter()
                .map(|span| span.range())
                .collect::<Vec<_>>(),
            [empty_use]
        );

        let shorthand = range_in(source, "Choice.Named { value }", "value");
        let shorthand_entities = model.entities_at(file, shorthand);
        assert!(
            shorthand_entities
                .iter()
                .any(|entity| matches!(entity, SemanticEntity::Member(_)))
        );
        assert!(
            shorthand_entities
                .iter()
                .any(|entity| matches!(entity, SemanticEntity::Name(ResolvedName::Local(_))))
        );
    }

    #[test]
    fn calls_report_closed_error_sets_in_canonical_member_order() {
        let source = "fn plain(): Int { 1 }\nfn one(): Int ! String { 1 }\nfn many(): Int ! (String | Bool) { 1 }\nfn impossible(): Int ! Never { 1 }\nfn calls() {\n    let a = plain()\n    let b = one()\n    let c = many()\n    let d = impossible()\n}\n";
        let output = compile(source);
        let model = output.semantic_model().unwrap();
        let file = FileId::from_index(0).unwrap();

        let cases = [
            ("let a = plain()", "plain()", Vec::<&str>::new()),
            ("let b = one()", "one()", vec!["String"]),
            ("let c = many()", "many()", vec!["Bool", "String"]),
            ("let d = impossible()", "impossible()", Vec::<&str>::new()),
        ];
        for (anchor, needle, expected) in cases {
            let range = range_in(source, anchor, needle);
            let errors = model.closed_call_errors_at(file, range).unwrap().unwrap();
            let names = errors
                .iter()
                .map(|error| model.canonical_type(*error).unwrap().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(names, expected);
            let (call, _) = model.call_expression_at(file, range).unwrap();
            assert!(model.call_signature(call).is_some());
        }
    }

    #[test]
    fn references_are_sorted_by_logical_source_identity_not_file_id() {
        let files = [
            (
                "main",
                "z-main.to",
                "import main.util\nfn main() {\n    let result = util.answer()\n}\n",
            ),
            (
                "b",
                "b.to",
                "import main.util\nfn from_b(): Int { util.answer() }\n",
            ),
            ("util", "util.to", "pub fn answer(): Int { 42 }\n"),
            (
                "a",
                "a.to",
                "import main.util\nfn from_a(): Int { util.answer() }\n",
            ),
        ];
        let mut sources = SourceDatabase::new();
        let source_id = SourceId::new("root:ordered-reference-test").unwrap();
        let mut file_ids = Vec::new();
        for (module, path, source) in files {
            file_ids.push(
                sources
                    .add(SourceInput::virtual_file(
                        source_id.clone(),
                        ModulePath::new(module).unwrap(),
                        LogicalPath::new(path).unwrap(),
                        Arc::<[u8]>::from(source.as_bytes().to_vec()),
                    ))
                    .unwrap(),
            );
        }
        let root = file_ids[0];
        let packages = PackageGraph::loose(&sources, root).unwrap();
        let output = execute(
            CompilationRequest::new(
                Operation::Check,
                Edition::V0_1,
                BuildTarget::vm_hosted(),
                HostProfile::Hosted,
                BTreeSet::new(),
                DiagnosticFormat::Json,
                SourceForm::Module,
                ResourceLimits::default(),
                packages,
                sources,
                root,
            )
            .unwrap(),
        )
        .unwrap();
        let model = output.semantic_model().unwrap();
        let answer = model
            .resolved()
            .symbols()
            .find(|symbol| symbol.name().as_str() == "answer")
            .unwrap()
            .id();
        let modules = model
            .references(&SemanticEntity::Name(ResolvedName::Symbol(answer)))
            .iter()
            .map(|span| model.sources().get(span.file()).unwrap().module().as_str())
            .collect::<Vec<_>>();
        assert_eq!(modules, ["a", "b", "main"]);
    }

    #[test]
    fn semantic_snapshots_are_available_at_each_completed_frontend_stage() {
        let syntax_error = compile("enum Empty {}\n");
        assert!(syntax_error.semantic_model().is_none());

        let resolution_error = compile("fn duplicate() {}\nfn duplicate() {}\n");
        let resolved = resolution_error
            .semantic_model()
            .expect("resolution produced a partial snapshot");
        assert!(resolved.hir().is_none());
        assert_eq!(resolved.resolved().symbols().count(), 1);

        let expression_error = compile("fn main() {\n    let flag: Bool = 1\n}\n");
        let checked = expression_error
            .semantic_model()
            .expect("type checking produced a partial snapshot");
        assert!(checked.hir().is_some());

        let formatted = compile_operation("fn main(){}\n", Operation::Format);
        assert!(formatted.semantic_model().is_none());
    }
}
