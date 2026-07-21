use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{DeclarationPath, ModuleId, Name, Namespace, PackageGraph, SymbolIdentity};
use crate::resolve::{
    LocalId, LocalKind, ResolvedEntity, ResolvedName, ResolvedProgram, SymbolId, SymbolKind,
};
use crate::source::{FileId, SourceDatabase, Span, TextRange};
use crate::syntax::ast::Expression as AstExpression;
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};
use crate::types::{
    FunctionParameter, FunctionType, IntrinsicType, ParameterMode, ScalarType, TypeId,
    TypeInterner, TypeKind, TypeSubstitution,
};

use super::termination::{TraitTerminationEdge, TraitTerminationError, analyze_trait_termination};
use super::{
    HirCallableId, HirCallableSignature, HirConstant, HirError, HirField, HirGenericParameter,
    HirImplementation, HirImplementationId, HirImplementationMethod,
    HirImplementationMethodContract, HirImplementationMethodId, HirNominalDefinition,
    HirNominalShape, HirOutput, HirParameter, HirPreludeTraitMethod, HirProgram,
    HirTraitConstructor, HirTraitDefinition, HirTraitIdentity, HirTraitMethod, HirTraitMethodKey,
    HirTraitReference, HirTypeDeclaration, HirTypeDeclarationKind, HirVariant, HirVariantPayload,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeLoweringLimits {
    pub max_type_nodes: u32,
    pub max_trait_obligations: u32,
    pub max_diagnostics: usize,
}

pub fn lower_types<'a>(
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: impl IntoIterator<Item = (FileId, &'a Parsed)>,
    resolved: &'a ResolvedProgram,
    limits: TypeLoweringLimits,
) -> Result<HirOutput, HirError> {
    let parsed = parsed.into_iter().collect::<BTreeMap<_, _>>();
    let mut lowerer = TypeLowerer {
        packages,
        sources,
        parsed,
        resolved,
        diagnostics: Vec::new(),
        max_diagnostics: limits.max_diagnostics,
        max_trait_termination_steps: u64::from(limits.max_trait_obligations),
        interner: TypeInterner::new(limits.max_type_nodes)?,
        sites: BTreeMap::new(),
        alias_dependencies: BTreeMap::new(),
        cyclic_aliases: BTreeSet::new(),
        alias_templates: BTreeMap::new(),
        declaration_environments: BTreeMap::new(),
        declaration_parameters: BTreeMap::new(),
        declarations: BTreeMap::new(),
        constants: BTreeMap::new(),
        callables: Vec::new(),
        implementation_sites: Vec::new(),
        implementations: Vec::new(),
        annotations: BTreeMap::new(),
        generic_types: BTreeMap::new(),
    };
    lowerer.index_declarations()?;
    lowerer.index_implementations()?;
    lowerer.analyze_aliases()?;
    lowerer.lower_aliases()?;
    lowerer.lower_declarations()?;
    lowerer.lower_remaining_source()?;
    lowerer.validate_productivity()?;
    lowerer.callables.sort_by_key(|callable| callable.id);
    Ok(HirOutput {
        program: HirProgram {
            interner: lowerer.interner,
            declarations: lowerer.declarations,
            constants: lowerer.constants,
            callables: lowerer.callables,
            implementations: lowerer.implementations,
            annotations: lowerer.annotations,
            expressions: Vec::new(),
            expression_flows: Vec::new(),
            expression_breaks: Vec::new(),
            member_references: Vec::new(),
            patterns: Vec::new(),
            bodies: BTreeMap::new(),
            local_types: lowerer.generic_types,
            discard_statuses: Vec::new(),
            expression_check_complete: false,
        },
        diagnostics: lowerer.diagnostics,
    })
}

#[derive(Debug, Clone, Copy)]
struct DeclarationSite<'a> {
    file: FileId,
    node: SyntaxNodeRef<'a>,
}

#[derive(Debug, Clone, Copy)]
struct ImplementationSite<'a> {
    file: FileId,
    node: SyntaxNodeRef<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoherenceConflictKind {
    Overlap,
    IteratorElement,
}

#[derive(Debug, Clone)]
struct ExpectedTraitMethod {
    name: Name,
    key: HirTraitMethodKey,
    declaration_span: Option<Span>,
    has_default: bool,
    requires_self_send: bool,
    signature: ExpectedTraitMethodSignature,
}

#[derive(Debug, Clone)]
enum ExpectedTraitMethodSignature {
    Source {
        callable: HirCallableSignature,
        fixed_arity: u32,
    },
    Concrete {
        function_type: TypeId,
        has_receiver: bool,
    },
}

#[derive(Debug, Clone, Default)]
struct TypeEnvironment {
    generics: BTreeMap<LocalId, TypeId>,
    contextual_self: Option<TypeId>,
    next_position: u32,
}

enum ProductivityTask {
    Type(TypeId),
    Nominal(SymbolId, Vec<TypeId>),
    All(Vec<TypeId>),
    Any(Vec<Vec<TypeId>>),
    CombineAll(usize),
    CombineAny(usize),
    ExitNominal(SymbolId),
}

struct TypeLowerer<'a> {
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: BTreeMap<FileId, &'a Parsed>,
    resolved: &'a ResolvedProgram,
    diagnostics: Vec<Diagnostic>,
    max_diagnostics: usize,
    max_trait_termination_steps: u64,
    interner: TypeInterner,
    sites: BTreeMap<SymbolId, DeclarationSite<'a>>,
    alias_dependencies: BTreeMap<SymbolId, Vec<SymbolId>>,
    cyclic_aliases: BTreeSet<SymbolId>,
    alias_templates: BTreeMap<SymbolId, TypeId>,
    declaration_environments: BTreeMap<SymbolId, TypeEnvironment>,
    declaration_parameters: BTreeMap<SymbolId, Vec<HirGenericParameter>>,
    declarations: BTreeMap<SymbolId, HirTypeDeclaration>,
    constants: BTreeMap<SymbolId, HirConstant>,
    callables: Vec<HirCallableSignature>,
    implementation_sites: Vec<ImplementationSite<'a>>,
    implementations: Vec<HirImplementation>,
    annotations: BTreeMap<(FileId, u32, u32), TypeId>,
    generic_types: BTreeMap<LocalId, TypeId>,
}

impl<'a> TypeLowerer<'a> {
    fn index_declarations(&mut self) -> Result<(), HirError> {
        let symbols_by_span = self
            .resolved
            .symbols()
            .filter(|symbol| symbol.identity().namespace() == Namespace::Type)
            .map(|symbol| {
                let span = symbol.span();
                (
                    (span.file(), span.range().start(), span.range().end()),
                    symbol.id(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let files = self.parsed.keys().copied().collect::<Vec<_>>();
        for file in files {
            let parsed = *self
                .parsed
                .get(&file)
                .expect("the file key came from the parsed source map");
            for node in parsed.cst().root_node().child_nodes() {
                if !matches!(
                    node.kind(),
                    SyntaxKind::TypeDecl
                        | SyntaxKind::AliasDecl
                        | SyntaxKind::EnumDecl
                        | SyntaxKind::TraitDecl
                ) {
                    continue;
                }
                let Some(token) = first_identifier(node) else {
                    continue;
                };
                let key = (file, token.range().start(), token.range().end());
                let Some(symbol) = symbols_by_span.get(&key).copied() else {
                    self.emit(
                        file,
                        token.range(),
                        "E1115",
                        "type declaration has no resolved symbol",
                        None,
                        None,
                    )?;
                    continue;
                };
                self.sites.insert(symbol, DeclarationSite { file, node });
            }
        }
        Ok(())
    }

    fn index_implementations(&mut self) -> Result<(), HirError> {
        for (file, parsed) in &self.parsed {
            self.implementation_sites.extend(
                parsed
                    .cst()
                    .root_node()
                    .child_nodes()
                    .filter(|node| node.kind() == SyntaxKind::ImplDecl)
                    .map(|node| ImplementationSite { file: *file, node }),
            );
        }
        self.implementation_sites.sort_by_key(|site| {
            let source = self
                .sources
                .get(site.file)
                .expect("parsed implementation files remain in the source database");
            (
                source.source_id().clone(),
                source.module().clone(),
                source.path().clone(),
                site.node.range().start(),
            )
        });
        Ok(())
    }

    fn analyze_aliases(&mut self) -> Result<(), HirError> {
        let aliases = self
            .sites
            .keys()
            .copied()
            .filter(|symbol| {
                self.resolved
                    .symbol(*symbol)
                    .is_some_and(|symbol| symbol.kind() == SymbolKind::Alias)
            })
            .collect::<Vec<_>>();
        for alias in &aliases {
            let site = self.sites[alias];
            let mut dependencies = site
                .node
                .descendant_tokens()
                .filter_map(|token| self.resolved.reference(site.file, token.range()))
                .filter_map(|reference| match reference.entity() {
                    ResolvedEntity::Name(ResolvedName::Symbol(symbol)) => Some(*symbol),
                    _ => None,
                })
                .filter(|symbol| {
                    self.resolved
                        .symbol(*symbol)
                        .is_some_and(|symbol| symbol.kind() == SymbolKind::Alias)
                })
                .collect::<Vec<_>>();
            dependencies.sort_unstable();
            dependencies.dedup();
            self.alias_dependencies.insert(*alias, dependencies);
        }

        for component in strongly_connected_components(&aliases, &self.alias_dependencies) {
            let cyclic = component.len() > 1
                || component.first().is_some_and(|alias| {
                    self.alias_dependencies
                        .get(alias)
                        .is_some_and(|dependencies| dependencies.contains(alias))
                });
            if !cyclic {
                continue;
            }
            self.cyclic_aliases.extend(component.iter().copied());
            let primary = component[0];
            let symbol = self
                .resolved
                .symbol(primary)
                .expect("alias sites refer to resolved symbols");
            let names = component
                .iter()
                .map(|symbol| {
                    self.resolved
                        .symbol(*symbol)
                        .expect("alias components contain resolved symbols")
                        .name()
                        .to_string()
                })
                .collect::<Vec<_>>();
            let mut related = Vec::new();
            for other in component.iter().skip(1) {
                related.push((
                    "alias in this cycle",
                    self.resolved
                        .symbol(*other)
                        .expect("alias components contain resolved symbols")
                        .span(),
                ));
            }
            self.emit(
                symbol.span().file(),
                symbol.span().range(),
                "E1106",
                format!("transparent aliases `{}` form a cycle", names.join("`, `")),
                Some(related),
                None,
            )?;
        }
        Ok(())
    }

    fn lower_aliases(&mut self) -> Result<(), HirError> {
        let error = self.interner.error();
        for alias in &self.cyclic_aliases {
            self.alias_templates.insert(*alias, error);
        }

        let acyclic = self
            .alias_dependencies
            .keys()
            .copied()
            .filter(|alias| !self.cyclic_aliases.contains(alias))
            .collect::<BTreeSet<_>>();
        let mut remaining = BTreeMap::<SymbolId, usize>::new();
        let mut users = BTreeMap::<SymbolId, Vec<SymbolId>>::new();
        for alias in &acyclic {
            let dependencies = self
                .alias_dependencies
                .get(alias)
                .into_iter()
                .flatten()
                .filter(|dependency| acyclic.contains(dependency))
                .copied()
                .collect::<Vec<_>>();
            remaining.insert(*alias, dependencies.len());
            for dependency in dependencies {
                users.entry(dependency).or_default().push(*alias);
            }
        }
        let mut ready = remaining
            .iter()
            .filter_map(|(alias, count)| (*count == 0).then_some(*alias))
            .collect::<BTreeSet<_>>();
        let mut lowered = 0_usize;
        while let Some(alias) = ready.pop_first() {
            self.lower_alias_template(alias)?;
            lowered += 1;
            for user in users.get(&alias).into_iter().flatten() {
                let count = remaining
                    .get_mut(user)
                    .expect("all alias users have a dependency count");
                *count -= 1;
                if *count == 0 {
                    ready.insert(*user);
                }
            }
        }
        debug_assert_eq!(lowered, acyclic.len());
        Ok(())
    }

    fn lower_alias_template(&mut self, symbol: SymbolId) -> Result<TypeId, HirError> {
        if let Some(template) = self.alias_templates.get(&symbol).copied() {
            return Ok(template);
        }
        let Some(site) = self.sites.get(&symbol).copied() else {
            return Ok(self.interner.error());
        };
        let (environment, _) = self.declaration_environment(symbol)?;
        let Some(target) = site
            .node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypeExpr)
        else {
            self.emit(
                site.file,
                site.node.range(),
                "E1115",
                "alias declaration has no target type",
                None,
                None,
            )?;
            let error = self.interner.error();
            self.alias_templates.insert(symbol, error);
            return Ok(error);
        };
        let template = self.lower_type_expr(site.file, target, &environment)?;
        self.alias_templates.insert(symbol, template);
        Ok(template)
    }

    fn declaration_environment(
        &mut self,
        symbol: SymbolId,
    ) -> Result<(TypeEnvironment, Vec<HirGenericParameter>), HirError> {
        if let (Some(environment), Some(parameters)) = (
            self.declaration_environments.get(&symbol),
            self.declaration_parameters.get(&symbol),
        ) {
            return Ok((environment.clone(), parameters.clone()));
        }
        let site = self.sites[&symbol];
        let groups = site
            .node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
            .collect::<Vec<_>>();
        let mut environment = TypeEnvironment::default();
        let (declarations, mut parameters) =
            self.declare_generics(site.file, &groups, &mut environment)?;
        if self
            .resolved
            .symbol(symbol)
            .is_some_and(|symbol| symbol.kind() == SymbolKind::Trait)
        {
            let self_type = self.interner.generic_parameter(environment.next_position)?;
            environment.contextual_self = Some(self_type);
            environment.next_position = environment.next_position.checked_add(1).ok_or(
                crate::types::TypeError::ResourceLimit {
                    limit: self.interner.len().try_into().unwrap_or(u32::MAX),
                },
            )?;
        }
        self.finish_generic_bounds(site.file, &declarations, &mut parameters, &environment)?;
        self.declaration_environments
            .insert(symbol, environment.clone());
        self.declaration_parameters
            .insert(symbol, parameters.clone());
        Ok((environment, parameters))
    }

    fn extend_generics(
        &mut self,
        file: FileId,
        groups: &[SyntaxNodeRef<'a>],
        environment: &mut TypeEnvironment,
    ) -> Result<Vec<HirGenericParameter>, HirError> {
        let (declarations, mut parameters) = self.declare_generics(file, groups, environment)?;
        self.finish_generic_bounds(file, &declarations, &mut parameters, environment)?;
        Ok(parameters)
    }

    fn declare_generics(
        &mut self,
        file: FileId,
        groups: &[SyntaxNodeRef<'a>],
        environment: &mut TypeEnvironment,
    ) -> Result<(Vec<SyntaxNodeRef<'a>>, Vec<HirGenericParameter>), HirError> {
        let declarations = groups
            .iter()
            .flat_map(|group| group.child_nodes())
            .filter(|child| child.kind() == SyntaxKind::GenericParam)
            .collect::<Vec<_>>();
        let mut parameters = Vec::with_capacity(declarations.len());
        for declaration in &declarations {
            let Some(token) = declaration
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
            else {
                continue;
            };
            let Some(local) = self.resolved.local_at(file, token.range()) else {
                self.emit(
                    file,
                    token.range(),
                    "E1115",
                    "generic parameter has no resolved binder",
                    None,
                    None,
                )?;
                continue;
            };
            if local.kind() != LocalKind::GenericParameter {
                self.emit(
                    file,
                    token.range(),
                    "E1115",
                    "type binder does not resolve to a generic parameter",
                    None,
                    None,
                )?;
                continue;
            }
            let position = environment.next_position;
            environment.next_position = environment.next_position.checked_add(1).ok_or(
                crate::types::TypeError::ResourceLimit {
                    limit: self.interner.len().try_into().unwrap_or(u32::MAX),
                },
            )?;
            let ty = self.interner.generic_parameter(position)?;
            environment.generics.insert(local.id(), ty);
            self.generic_types.insert(local.id(), ty);
            parameters.push(HirGenericParameter {
                local: local.id(),
                position,
                bounds: Vec::new(),
            });
        }
        Ok((declarations, parameters))
    }

    fn finish_generic_bounds(
        &mut self,
        file: FileId,
        declarations: &[SyntaxNodeRef<'a>],
        parameters: &mut [HirGenericParameter],
        environment: &TypeEnvironment,
    ) -> Result<(), HirError> {
        for (declaration, parameter) in declarations.iter().zip(parameters) {
            if let Some(bound) = declaration
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::GenericBound)
            {
                parameter.bounds = self.lower_generic_bound(file, bound, environment)?;
            }
        }
        Ok(())
    }

    fn lower_generic_bound(
        &mut self,
        file: FileId,
        bound: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<Vec<HirTraitReference>, HirError> {
        let mut references = Vec::new();
        let mut seen = BTreeSet::new();
        for path in bound
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::TypePath)
        {
            let reference = self.lower_trait_path(file, path, environment)?;
            let key = (reference.constructor.clone(), reference.arguments.clone());
            if !seen.insert(key) {
                self.emit(
                    file,
                    path.range(),
                    "E1115",
                    "a generic bound cannot repeat the same trait",
                    None,
                    None,
                )?;
            } else {
                references.push(reference);
            }
        }
        references.sort_by(|left, right| {
            (&left.constructor, &left.arguments).cmp(&(&right.constructor, &right.arguments))
        });
        Ok(references)
    }

    fn lower_trait_path(
        &mut self,
        file: FileId,
        path: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<HirTraitReference, HirError> {
        let arguments = self.lower_generic_arguments(file, path, environment)?;
        let Some((token, resolved)) = self.terminal_resolution(file, path)? else {
            self.emit(
                file,
                path.range(),
                "E1115",
                "trait bound has no resolved constructor",
                None,
                None,
            )?;
            return Ok(HirTraitReference {
                constructor: HirTraitConstructor::Prelude(Name::new("Display").unwrap()),
                arguments: Vec::new(),
            });
        };
        let actual = arguments.len();
        let constructor = match resolved {
            ResolvedName::Symbol(symbol) => {
                let declaration = self
                    .resolved
                    .symbol(symbol)
                    .expect("resolved symbol references are valid");
                if declaration.kind() != SymbolKind::Trait {
                    self.emit(
                        file,
                        token.range(),
                        "E1115",
                        format!("`{}` is a value type, not a trait", declaration.name()),
                        None,
                        None,
                    )?;
                }
                self.check_arity(
                    file,
                    token.range(),
                    declaration.name().as_str(),
                    declaration.generic_arity() as usize,
                    actual,
                )?;
                HirTraitConstructor::Symbol(symbol)
            }
            ResolvedName::Prelude { name, .. } => {
                let expected = prelude_trait_arity(name.as_str());
                let Some(expected) = expected else {
                    self.emit(
                        file,
                        token.range(),
                        "E1115",
                        format!("`{name}` is a value type, not a trait"),
                        None,
                        None,
                    )?;
                    return Ok(HirTraitReference {
                        constructor: HirTraitConstructor::Prelude(name),
                        arguments,
                    });
                };
                self.check_arity(file, token.range(), name.as_str(), expected, actual)?;
                HirTraitConstructor::Prelude(name)
            }
            ResolvedName::External { module, name, .. } => {
                let identity = self.packages.symbol_identity(
                    module,
                    Namespace::Type,
                    DeclarationPath::single(name),
                )?;
                HirTraitConstructor::External(identity)
            }
            ResolvedName::Local(_) | ResolvedName::ContextualSelf | ResolvedName::Receiver => {
                self.emit(
                    file,
                    token.range(),
                    "E1115",
                    "a type parameter or `Self` cannot be used as a trait constructor",
                    None,
                    None,
                )?;
                HirTraitConstructor::Prelude(Name::new("Display").unwrap())
            }
        };
        Ok(HirTraitReference {
            constructor,
            arguments,
        })
    }

    fn lower_type_expr(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        debug_assert_eq!(node.kind(), SyntaxKind::TypeExpr);
        let key = (file, node.range().start(), node.range().end());
        if let Some(ty) = self.annotations.get(&key).copied() {
            return Ok(ty);
        }
        let Some(union) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::UnionType)
        else {
            let error = self.interner.error();
            self.annotations.insert(key, error);
            return Ok(error);
        };
        let ty = self.lower_union_type(file, union, environment)?;
        self.annotations.insert(key, ty);
        Ok(ty)
    }

    fn lower_union_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let mut written_members = Vec::new();
        for child in node
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::ResultType)
        {
            written_members.push(self.lower_result_type(file, child, environment)?);
        }
        if self.types_have_recovery(written_members.iter().copied())? {
            return Ok(self.interner.error());
        }
        if written_members.len() <= 1 {
            return Ok(written_members
                .first()
                .copied()
                .unwrap_or_else(|| self.interner.error()));
        }

        let mut flattened = Vec::new();
        for member in &written_members {
            self.flatten_union_members(*member, &mut flattened)?;
        }
        let never = self.interner.scalar(ScalarType::Never);
        flattened.retain(|member| *member != never);
        for member in &flattened {
            if !self.valid_union_member(*member)? {
                let actual = self
                    .interner
                    .canonical(*member)
                    .unwrap_or_else(|_| "<recovery>".into());
                self.emit(
                    file,
                    node.range(),
                    "E1115",
                    format!(
                        "structural union member `{actual}` has no canonical runtime discriminator"
                    ),
                    None,
                    None,
                )?;
            }
        }
        let mut unique = flattened;
        unique.sort_by_key(|ty| self.interner.canonical(*ty).unwrap_or_default());
        unique.dedup();
        'pairs: for left in 0..unique.len() {
            for right in left + 1..unique.len() {
                if self
                    .interner
                    .first_order_unifiable(unique[left], unique[right])?
                {
                    let left_name = self.interner.canonical(unique[left]).unwrap_or_default();
                    let right_name = self.interner.canonical(unique[right]).unwrap_or_default();
                    self.emit(
                        file,
                        node.range(),
                        "E1115",
                        format!(
                            "union members `{left_name}` and `{right_name}` overlap for a generic substitution"
                        ),
                        None,
                        None,
                    )?;
                    break 'pairs;
                }
            }
        }
        Ok(self.interner.union(written_members)?)
    }

    fn lower_result_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let operands = node.child_nodes().collect::<Vec<_>>();
        let has_bang = node
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Bang);
        match (has_bang, operands.as_slice()) {
            (false, [value]) => self.lower_type_operand(file, *value, environment),
            (true, [error]) => {
                let unit = self.interner.scalar(ScalarType::Unit);
                let error = self.lower_type_operand(file, *error, environment)?;
                if self.type_has_recovery(error)? {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.result(unit, error)?)
                }
            }
            (true, [success, error]) => {
                let success = self.lower_type_operand(file, *success, environment)?;
                let error = self.lower_type_operand(file, *error, environment)?;
                if self.type_has_recovery(success)? || self.type_has_recovery(error)? {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.result(success, error)?)
                }
            }
            _ => Ok(self.interner.error()),
        }
    }

    fn lower_type_operand(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        match node.kind() {
            SyntaxKind::OptionalType => self.lower_optional_type(file, node, environment),
            SyntaxKind::GroupType => {
                let Some(inner) = node
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::TypeExpr)
                else {
                    return Ok(self.interner.error());
                };
                self.lower_type_expr(file, inner, environment)
            }
            _ => Ok(self.interner.error()),
        }
    }

    fn lower_optional_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let Some(primary) = node.child_nodes().next() else {
            return Ok(self.interner.error());
        };
        let item = self.lower_primary_type(file, primary, environment)?;
        if node
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Question)
        {
            if self.type_has_recovery(item)? {
                Ok(self.interner.error())
            } else {
                Ok(self.interner.option(item)?)
            }
        } else {
            Ok(item)
        }
    }

    fn lower_primary_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        match node.kind() {
            SyntaxKind::PathType => self.lower_path_type(file, node, environment),
            SyntaxKind::TupleType => {
                let mut items = Vec::new();
                for item in node
                    .child_nodes()
                    .filter(|child| child.kind() == SyntaxKind::TypeExpr)
                {
                    items.push(self.lower_type_expr(file, item, environment)?);
                }
                if self.types_have_recovery(items.iter().copied())? {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.tuple(items)?)
                }
            }
            SyntaxKind::GroupType => {
                let Some(inner) = node
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::TypeExpr)
                else {
                    return Ok(self.interner.error());
                };
                self.lower_type_expr(file, inner, environment)
            }
            SyntaxKind::FunctionType => self.lower_function_type(file, node, environment),
            _ => Ok(self.interner.error()),
        }
    }

    fn lower_path_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let Some(path) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypePath)
        else {
            return Ok(self.interner.error());
        };
        self.lower_type_path(file, path, environment)
    }

    fn lower_type_path(
        &mut self,
        file: FileId,
        path: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let arguments = self.lower_generic_arguments(file, path, environment)?;
        let Some((token, resolved)) = self.terminal_resolution(file, path)? else {
            return Ok(self.interner.error());
        };
        self.lower_resolved_type(file, token.range(), resolved, arguments, environment)
    }

    fn lower_generic_arguments(
        &mut self,
        file: FileId,
        path: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<Vec<TypeId>, HirError> {
        let Some(arguments) = path
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::GenericArgs)
        else {
            return Ok(Vec::new());
        };
        arguments
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::TypeExpr)
            .map(|argument| self.lower_type_expr(file, argument, environment))
            .collect()
    }

    fn terminal_resolution(
        &mut self,
        file: FileId,
        path: SyntaxNodeRef<'a>,
    ) -> Result<Option<(SyntaxTokenRef<'a>, ResolvedName)>, HirError> {
        let identifiers = path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        if identifiers.len() > 2 {
            self.emit(
                file,
                path.range(),
                "E1115",
                "a source type path is either unqualified or `module.Type`",
                None,
                None,
            )?;
        }
        for token in identifiers {
            let Some(reference) = self.resolved.reference(file, token.range()) else {
                continue;
            };
            let name = match reference.entity() {
                ResolvedEntity::Name(name) => Some(name.clone()),
                ResolvedEntity::ContextualCandidates { type_name, .. } => Some(type_name.clone()),
                ResolvedEntity::Module(_) => None,
            };
            if let Some(name) = name {
                return Ok(Some((token, name)));
            }
        }
        Ok(None)
    }

    fn lower_resolved_type(
        &mut self,
        file: FileId,
        range: TextRange,
        resolved: ResolvedName,
        arguments: Vec<TypeId>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        match resolved {
            ResolvedName::Symbol(symbol) => {
                let declaration = self
                    .resolved
                    .symbol(symbol)
                    .expect("resolved symbol references are valid");
                let expected = declaration.generic_arity() as usize;
                if !self.check_arity(
                    file,
                    range,
                    declaration.name().as_str(),
                    expected,
                    arguments.len(),
                )? {
                    return Ok(self.interner.error());
                }
                match declaration.kind() {
                    SymbolKind::Alias => {
                        let template = self
                            .alias_templates
                            .get(&symbol)
                            .copied()
                            .unwrap_or_else(|| self.interner.error());
                        if template == self.interner.error() {
                            return Ok(template);
                        }
                        if self.types_have_recovery(arguments.iter().copied())? {
                            return Ok(self.interner.error());
                        }
                        if arguments.is_empty() {
                            Ok(template)
                        } else {
                            Ok(TypeSubstitution::new(arguments)
                                .apply(&mut self.interner, template)?)
                        }
                    }
                    SymbolKind::Type | SymbolKind::Enum => {
                        if self.types_have_recovery(arguments.iter().copied())? {
                            Ok(self.interner.error())
                        } else {
                            Ok(self
                                .interner
                                .nominal(declaration.identity().clone(), arguments)?)
                        }
                    }
                    SymbolKind::Trait => {
                        self.emit(
                            file,
                            range,
                            "E1110",
                            format!(
                                "trait `{}` cannot be used as the type of a value",
                                declaration.name()
                            ),
                            None,
                            None,
                        )?;
                        Ok(self.interner.error())
                    }
                    SymbolKind::Constant
                    | SymbolKind::Function
                    | SymbolKind::NewtypeConstructor => {
                        self.emit(
                            file,
                            range,
                            "E1115",
                            "value declaration used as a type constructor",
                            None,
                            None,
                        )?;
                        Ok(self.interner.error())
                    }
                }
            }
            ResolvedName::Local(local) => {
                if !arguments.is_empty() {
                    self.check_arity(
                        file,
                        range,
                        self.resolved
                            .local(local)
                            .map_or("type parameter", |local| local.name().as_str()),
                        0,
                        arguments.len(),
                    )?;
                    return Ok(self.interner.error());
                }
                if let Some(ty) = environment.generics.get(&local).copied() {
                    Ok(ty)
                } else {
                    self.emit(
                        file,
                        range,
                        "E1115",
                        "generic parameter is outside the active type binder",
                        None,
                        None,
                    )?;
                    Ok(self.interner.error())
                }
            }
            ResolvedName::ContextualSelf => {
                if !arguments.is_empty() {
                    self.check_arity(file, range, "Self", 0, arguments.len())?;
                    return Ok(self.interner.error());
                }
                if let Some(self_type) = environment.contextual_self {
                    Ok(self_type)
                } else {
                    self.emit(
                        file,
                        range,
                        "E1115",
                        "`Self` is not yet defined in this type position",
                        None,
                        None,
                    )?;
                    Ok(self.interner.error())
                }
            }
            ResolvedName::Prelude { name, .. } => {
                self.lower_prelude_type(file, range, name, arguments)
            }
            ResolvedName::External {
                module,
                namespace,
                name,
            } => {
                if namespace != Namespace::Type {
                    self.emit(
                        file,
                        range,
                        "E1115",
                        "external value declaration used as a type",
                        None,
                        None,
                    )?;
                    return Ok(self.interner.error());
                }
                let identity = self.packages.symbol_identity(
                    module,
                    Namespace::Type,
                    DeclarationPath::single(name),
                )?;
                if self.types_have_recovery(arguments.iter().copied())? {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.nominal(identity, arguments)?)
                }
            }
            ResolvedName::Receiver => {
                self.emit(
                    file,
                    range,
                    "E1115",
                    "receiver value `self` cannot be used as a type",
                    None,
                    None,
                )?;
                Ok(self.interner.error())
            }
        }
    }

    fn lower_prelude_type(
        &mut self,
        file: FileId,
        range: TextRange,
        name: Name,
        arguments: Vec<TypeId>,
    ) -> Result<TypeId, HirError> {
        let has_recovery = self.types_have_recovery(arguments.iter().copied())?;
        if let Some(scalar) = self.interner.named_scalar(name.as_str()) {
            if !self.check_arity(file, range, name.as_str(), 0, arguments.len())? {
                return Ok(self.interner.error());
            }
            return Ok(scalar);
        }
        match name.as_str() {
            "Option" => {
                if !self.check_arity(file, range, name.as_str(), 1, arguments.len())? {
                    return Ok(self.interner.error());
                }
                if has_recovery {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.option(arguments[0])?)
                }
            }
            "Result" => {
                if !self.check_arity(file, range, name.as_str(), 2, arguments.len())? {
                    return Ok(self.interner.error());
                }
                if has_recovery {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.result(arguments[0], arguments[1])?)
                }
            }
            _ if intrinsic_type(name.as_str()).is_some() => {
                let constructor = intrinsic_type(name.as_str())
                    .expect("the guarded prelude name is an intrinsic type");
                if !self.check_arity(
                    file,
                    range,
                    name.as_str(),
                    constructor.arity(),
                    arguments.len(),
                )? {
                    return Ok(self.interner.error());
                }
                if has_recovery {
                    Ok(self.interner.error())
                } else {
                    Ok(self.interner.intrinsic(constructor, arguments)?)
                }
            }
            _ if prelude_trait_arity(name.as_str()).is_some() => {
                self.emit(
                    file,
                    range,
                    "E1110",
                    format!("trait `{name}` cannot be used as the type of a value"),
                    None,
                    None,
                )?;
                Ok(self.interner.error())
            }
            _ => {
                self.emit(
                    file,
                    range,
                    "E1115",
                    format!("prelude name `{name}` is not a value type"),
                    None,
                    None,
                )?;
                Ok(self.interner.error())
            }
        }
    }

    fn check_arity(
        &mut self,
        file: FileId,
        range: TextRange,
        constructor: &str,
        expected: usize,
        actual: usize,
    ) -> Result<bool, HirError> {
        if expected == actual {
            return Ok(true);
        }
        self.emit(
            file,
            range,
            "E1104",
            format!(
                "type constructor `{constructor}` requires {expected} generic arguments, got {actual}"
            ),
            None,
            Some((expected.to_string(), actual.to_string())),
        )?;
        Ok(false)
    }

    fn lower_function_type(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let mut parameters = Vec::new();
        let mut variadic = None;
        if let Some(list) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::FunctionTypeList)
        {
            let items = list
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::FunctionTypeItem)
                .collect::<Vec<_>>();
            for (index, item) in items.iter().enumerate() {
                let Some(ty_node) = item
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::TypeExpr)
                else {
                    continue;
                };
                let ty = self.lower_type_expr(file, ty_node, environment)?;
                if item
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Ellipsis)
                {
                    if variadic.is_some() || index + 1 != items.len() {
                        self.emit(
                            file,
                            item.range(),
                            "E1115",
                            "a variadic function-type item must be unique and last",
                            None,
                            None,
                        )?;
                    }
                    variadic = Some(ty);
                } else {
                    parameters.push(FunctionParameter::new(parameter_mode(*item), ty));
                }
            }
        }
        let outcome = if let Some(annotation) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::OutcomeAnnotation)
        {
            if let Some(ty) = annotation
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
            {
                self.lower_type_expr(file, ty, environment)?
            } else {
                self.interner.error()
            }
        } else {
            self.interner.scalar(ScalarType::Unit)
        };
        if self.types_have_recovery(
            parameters
                .iter()
                .map(FunctionParameter::ty)
                .chain(variadic)
                .chain([outcome]),
        )? {
            Ok(self.interner.error())
        } else {
            Ok(self.interner.function(FunctionType::new(
                has_direct_token(node, TokenKind::Async),
                has_direct_token(node, TokenKind::Unsafe),
                parameters,
                variadic,
                outcome,
            ))?)
        }
    }

    fn flatten_union_members(&self, ty: TypeId, output: &mut Vec<TypeId>) -> Result<(), HirError> {
        if let TypeKind::Union(members) = self.interner.kind(ty)? {
            output.extend(members.iter().copied());
        } else {
            output.push(ty);
        }
        Ok(())
    }

    fn valid_union_member(&self, ty: TypeId) -> Result<bool, HirError> {
        Ok(matches!(
            self.interner.kind(ty)?,
            TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::Nominal { .. }
                | TypeKind::Option(_)
                | TypeKind::Result { .. }
                | TypeKind::Intrinsic { .. }
        ))
    }

    fn type_has_recovery(&self, root: TypeId) -> Result<bool, HirError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.interner.kind(ty)? {
                TypeKind::Error => return Ok(true),
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Intrinsic { arguments, .. }
                | TypeKind::Generated { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters().iter().map(FunctionParameter::ty));
                    pending.extend(function.variadic());
                    pending.push(function.outcome());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Scalar(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(false)
    }

    fn types_have_recovery(
        &self,
        types: impl IntoIterator<Item = TypeId>,
    ) -> Result<bool, HirError> {
        for ty in types {
            if self.type_has_recovery(ty)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn lower_declarations(&mut self) -> Result<(), HirError> {
        let symbols = self.sites.keys().copied().collect::<Vec<_>>();
        for symbol_id in symbols {
            let site = self.sites[&symbol_id];
            let symbol = self
                .resolved
                .symbol(symbol_id)
                .expect("declaration sites contain valid symbols");
            let kind = symbol.kind();
            let span = symbol.span();
            let identity = symbol.identity().clone();
            let (environment, parameters) = self.declaration_environment(symbol_id)?;
            let declaration_kind = match kind {
                SymbolKind::Alias => HirTypeDeclarationKind::Alias {
                    target: {
                        let target = self
                            .alias_templates
                            .get(&symbol_id)
                            .copied()
                            .unwrap_or_else(|| self.interner.error());
                        if let Some(node) = site
                            .node
                            .child_nodes()
                            .find(|child| child.kind() == SyntaxKind::TypeExpr)
                        {
                            self.annotations
                                .entry((site.file, node.range().start(), node.range().end()))
                                .or_insert(target);
                        }
                        target
                    },
                },
                SymbolKind::Type => {
                    let arguments = parameters
                        .iter()
                        .map(|parameter| self.interner.generic_parameter(parameter.position))
                        .collect::<Result<Vec<_>, _>>()?;
                    let self_type = self.interner.nominal(identity, arguments)?;
                    let shape = if let Some(body) = site
                        .node
                        .child_nodes()
                        .find(|child| child.kind() == SyntaxKind::RecordBody)
                    {
                        HirNominalShape::Record {
                            fields: self.lower_record_fields(site.file, body, &environment)?,
                        }
                    } else if let Some(underlying) = site
                        .node
                        .child_nodes()
                        .find(|child| child.kind() == SyntaxKind::TypeExpr)
                    {
                        HirNominalShape::Newtype {
                            underlying: self.lower_type_expr(
                                site.file,
                                underlying,
                                &environment,
                            )?,
                        }
                    } else {
                        self.emit(
                            site.file,
                            site.node.range(),
                            "E1115",
                            "nominal type declaration has no representation",
                            None,
                            None,
                        )?;
                        HirNominalShape::Newtype {
                            underlying: self.interner.error(),
                        }
                    };
                    HirTypeDeclarationKind::Nominal(HirNominalDefinition { self_type, shape })
                }
                SymbolKind::Enum => {
                    let arguments = parameters
                        .iter()
                        .map(|parameter| self.interner.generic_parameter(parameter.position))
                        .collect::<Result<Vec<_>, _>>()?;
                    let self_type = self.interner.nominal(identity, arguments)?;
                    let variants = self.lower_variants(site.file, site.node, &environment)?;
                    HirTypeDeclarationKind::Nominal(HirNominalDefinition {
                        self_type,
                        shape: HirNominalShape::Enum { variants },
                    })
                }
                SymbolKind::Trait => {
                    let self_type = environment
                        .contextual_self
                        .expect("trait environments always declare contextual Self");
                    let mut methods = site
                        .node
                        .child_nodes()
                        .filter(|child| child.kind() == SyntaxKind::TraitMethod)
                        .filter_map(|method| {
                            let name = first_identifier(method)?;
                            let member = self.resolved.member_at(site.file, name.range())?;
                            Some(HirTraitMethod {
                                member: member.id(),
                                has_default: method
                                    .child_nodes()
                                    .any(|child| child.kind() == SyntaxKind::Block),
                                requires_self_send: has_direct_token(method, TokenKind::Async)
                                    && callable_has_receiver(method),
                            })
                        })
                        .collect::<Vec<_>>();
                    methods.sort_by_key(HirTraitMethod::member);
                    HirTypeDeclarationKind::Trait(HirTraitDefinition { self_type, methods })
                }
                SymbolKind::Constant | SymbolKind::Function | SymbolKind::NewtypeConstructor => {
                    continue;
                }
            };
            self.declarations.insert(
                symbol_id,
                HirTypeDeclaration {
                    symbol: symbol_id,
                    span,
                    parameters,
                    kind: declaration_kind,
                },
            );
        }
        Ok(())
    }

    fn lower_record_fields(
        &mut self,
        file: FileId,
        body: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<Vec<HirField>, HirError> {
        let mut fields = Vec::new();
        for field in body
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::RecordField)
        {
            let Some(name) = field_name_token(field) else {
                continue;
            };
            let Some(member) = self.resolved.member_at(file, name.range()) else {
                self.emit(
                    file,
                    name.range(),
                    "E1115",
                    "record field has no resolved member identity",
                    None,
                    None,
                )?;
                continue;
            };
            let Some(ty_node) = field
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
            else {
                continue;
            };
            fields.push(HirField {
                member: member.id(),
                ty: self.lower_type_expr(file, ty_node, environment)?,
            });
        }
        Ok(fields)
    }

    fn lower_variants(
        &mut self,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<Vec<HirVariant>, HirError> {
        let mut variants = Vec::new();
        for variant in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::EnumVariant)
        {
            let Some(name) = first_identifier(variant) else {
                continue;
            };
            let Some(member) = self.resolved.member_at(file, name.range()) else {
                self.emit(
                    file,
                    name.range(),
                    "E1115",
                    "enum variant has no resolved member identity",
                    None,
                    None,
                )?;
                continue;
            };
            let payload = if let Some(tuple) = variant
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TuplePayload)
            {
                let mut items = Vec::new();
                for item in tuple
                    .child_nodes()
                    .filter(|child| child.kind() == SyntaxKind::TypeExpr)
                {
                    items.push(self.lower_type_expr(file, item, environment)?);
                }
                HirVariantPayload::Tuple(items)
            } else if let Some(record) = variant
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::RecordBody)
            {
                HirVariantPayload::Record(self.lower_record_fields(file, record, environment)?)
            } else {
                HirVariantPayload::Unit
            };
            variants.push(HirVariant {
                member: member.id(),
                payload,
            });
        }
        Ok(variants)
    }

    fn lower_remaining_source(&mut self) -> Result<(), HirError> {
        let files = self.parsed.keys().copied().collect::<Vec<_>>();
        for file in files {
            let root = self.parsed[&file].cst().root_node();
            for node in root.child_nodes() {
                match node.kind() {
                    SyntaxKind::ConstDecl => self.lower_constant_declaration(file, node)?,
                    SyntaxKind::FunctionDecl => self.lower_function_declaration(file, node)?,
                    SyntaxKind::TraitDecl => self.lower_trait_declaration(file, node)?,
                    SyntaxKind::ImplDecl => {}
                    SyntaxKind::TypeDecl | SyntaxKind::AliasDecl | SyntaxKind::EnumDecl => {}
                    SyntaxKind::ImportDecl => {}
                    _ => self.lower_annotation_tree(file, node, &TypeEnvironment::default())?,
                }
            }
        }
        let sites = self.implementation_sites.clone();
        for (index, site) in sites.into_iter().enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| crate::types::TypeError::ResourceLimit { limit: u32::MAX })?;
            self.lower_implementation(HirImplementationId(index), site.file, site.node)?;
        }
        let diagnostics_before_coherence = self.diagnostics.len();
        self.validate_implementation_coherence()?;
        if self.diagnostics.len() == diagnostics_before_coherence {
            self.validate_trait_termination()?;
        }
        Ok(())
    }

    fn lower_constant_declaration(
        &mut self,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), HirError> {
        let Some(name) = first_identifier(declaration) else {
            return Ok(());
        };
        let Some(symbol) = self
            .resolved
            .symbols()
            .find(|symbol| symbol.span().file() == file && symbol.span().range() == name.range())
        else {
            self.emit(
                file,
                name.range(),
                "E1115",
                "constant declaration has no resolved symbol identity",
                None,
                None,
            )?;
            return Ok(());
        };
        let symbol_id = symbol.id();
        let symbol_span = symbol.span();
        let is_public = symbol.visibility() == crate::resolve::Visibility::Public;
        let declared_type = if let Some(annotation) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypeExpr)
        {
            Some(self.lower_type_expr(file, annotation, &TypeEnvironment::default())?)
        } else {
            if is_public {
                self.emit(
                    file,
                    name.range(),
                    "E1115",
                    "a public constant must declare its type",
                    None,
                    None,
                )?;
            }
            None
        };
        let Some(initializer) = declaration.child_nodes().find_map(AstExpression::cast) else {
            self.emit(
                file,
                declaration.range(),
                "E1115",
                "constant declaration has no initializer expression",
                None,
                None,
            )?;
            return Ok(());
        };
        self.constants.insert(
            symbol_id,
            HirConstant {
                symbol: symbol_id,
                span: symbol_span,
                declared_type,
                initializer: self.sources.span(file, initializer.syntax().range())?,
                ty: None,
                value: None,
                evaluated: None,
            },
        );
        Ok(())
    }

    fn lower_function_declaration(
        &mut self,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), HirError> {
        let Some(head) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::FunctionHead)
        else {
            return Ok(());
        };
        let identifiers = head
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let Some(name) = identifiers.last().copied() else {
            return Ok(());
        };
        let groups = head
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
            .collect::<Vec<_>>();
        let mut environment = TypeEnvironment::default();
        let (generic_declarations, mut generics) =
            self.declare_generics(file, &groups, &mut environment)?;
        if identifiers.len() == 2 {
            let owner_token = identifiers[0];
            let dot = head
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Dot)
                .expect("an inherent function head contains a dot");
            let owner_groups = groups
                .iter()
                .copied()
                .filter(|group| group.range().end() <= dot.range().start())
                .collect::<Vec<_>>();
            let owner_arguments =
                generic_group_arguments(file, &owner_groups, &environment, self.resolved);
            if let Some(resolved) = self.resolved_name_at(file, owner_token) {
                environment.contextual_self = Some(self.lower_resolved_type(
                    file,
                    owner_token.range(),
                    resolved,
                    owner_arguments,
                    &environment,
                )?);
            }
        }
        self.finish_generic_bounds(file, &generic_declarations, &mut generics, &environment)?;

        let id = if identifiers.len() == 1 {
            self.resolved
                .symbols()
                .find(|symbol| {
                    symbol.span().file() == file && symbol.span().range() == name.range()
                })
                .map(|symbol| HirCallableId::Symbol(symbol.id()))
        } else {
            self.resolved
                .member_at(file, name.range())
                .map(|member| HirCallableId::Member(member.id()))
        };
        let Some(id) = id else {
            self.emit(
                file,
                name.range(),
                "E1115",
                "function declaration has no resolved callable identity",
                None,
                None,
            )?;
            return Ok(());
        };
        self.lower_callable(file, declaration, name.range(), id, environment, generics)
    }

    fn lower_trait_declaration(
        &mut self,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), HirError> {
        let Some(name) = first_identifier(declaration) else {
            return Ok(());
        };
        let Some(symbol) = self
            .resolved
            .symbols()
            .find(|symbol| symbol.span().file() == file && symbol.span().range() == name.range())
            .map(|symbol| symbol.id())
        else {
            return Ok(());
        };
        let (outer, outer_parameters) = self.declaration_environment(symbol)?;
        for method in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::TraitMethod)
        {
            let Some(method_name) = first_identifier(method) else {
                continue;
            };
            let Some(member) = self.resolved.member_at(file, method_name.range()) else {
                continue;
            };
            let groups = method
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::GenericParams)
                .collect::<Vec<_>>();
            let mut environment = outer.clone();
            let mut generics = outer_parameters.clone();
            generics.extend(self.extend_generics(file, &groups, &mut environment)?);
            self.lower_callable(
                file,
                method,
                method_name.range(),
                HirCallableId::Member(member.id()),
                environment,
                generics,
            )?;
        }
        Ok(())
    }

    fn lower_implementation(
        &mut self,
        id: HirImplementationId,
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), HirError> {
        let diagnostics_before = self.diagnostics.len();
        let groups = declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
            .collect::<Vec<_>>();
        let mut environment = TypeEnvironment::default();
        let (generic_declarations, mut outer_parameters) =
            self.declare_generics(file, &groups, &mut environment)?;
        let target = if let Some(target) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypeExpr)
        {
            self.lower_type_expr(file, target, &environment)?
        } else {
            self.interner.error()
        };
        environment.contextual_self = Some(target);
        let trait_reference = if let Some(trait_path) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypePath)
        {
            self.lower_trait_path(file, trait_path, &environment)?
        } else {
            HirTraitReference {
                constructor: HirTraitConstructor::Prelude(Name::new("Display").unwrap()),
                arguments: Vec::new(),
            }
        };
        self.finish_generic_bounds(
            file,
            &generic_declarations,
            &mut outer_parameters,
            &environment,
        )?;
        let mut methods = Vec::new();
        for method in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::ImplementationMethod)
        {
            let Some(method_name) = first_identifier(method) else {
                continue;
            };
            let method_index = u32::try_from(methods.len())
                .map_err(|_| crate::types::TypeError::ResourceLimit { limit: u32::MAX })?;
            let method_id = HirImplementationMethodId {
                implementation: id,
                index: method_index,
            };
            let method_groups = method
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::GenericParams)
                .collect::<Vec<_>>();
            let mut method_environment = environment.clone();
            let mut generics = outer_parameters.clone();
            generics.extend(self.extend_generics(file, &method_groups, &mut method_environment)?);
            self.lower_callable(
                file,
                method,
                method_name.range(),
                HirCallableId::Implementation(method_id),
                method_environment,
                generics,
            )?;
            methods.push(HirImplementationMethod {
                id: method_id,
                span: self.sources.span(file, method_name.range())?,
                name: Name::new(
                    method_name
                        .token()
                        .normalized_identifier()
                        .expect("implementation method names are identifiers"),
                )
                .expect("resolved implementation method names are valid"),
                contract: None,
            });
        }
        let mut implementation = HirImplementation {
            id,
            span: self.sources.span(file, declaration.range())?,
            module: self.packages.module_for_file(self.sources, file)?,
            parameters: outer_parameters,
            trait_reference,
            target,
            methods,
            contract_complete: false,
            requires_self_send: false,
        };
        self.validate_implementation_contract(&mut implementation)?;
        implementation.contract_complete &= self.diagnostics.len() == diagnostics_before;
        self.implementations.push(implementation);
        Ok(())
    }

    fn validate_implementation_contract(
        &mut self,
        implementation: &mut HirImplementation,
    ) -> Result<(), HirError> {
        let diagnostics_before = self.diagnostics.len();
        self.validate_implementation_binders(implementation)?;
        self.validate_orphan_rule(implementation)?;
        let Some(expected_methods) = self.expected_trait_methods(implementation)? else {
            return Ok(());
        };
        implementation.requires_self_send = expected_methods
            .iter()
            .any(|method| method.requires_self_send);

        let by_name = expected_methods
            .iter()
            .cloned()
            .map(|method| (method.name.clone(), method))
            .collect::<BTreeMap<_, _>>();
        let mut provided = BTreeSet::new();
        for method_index in 0..implementation.methods.len() {
            let name = implementation.methods[method_index].name.clone();
            let span = implementation.methods[method_index].span;
            let Some(expected) = by_name.get(&name) else {
                self.emit(
                    span.file(),
                    span.range(),
                    "E1114",
                    format!("implementation declares extra method `{name}`"),
                    None,
                    None,
                )?;
                continue;
            };
            if !provided.insert(name.clone()) {
                self.emit(
                    span.file(),
                    span.range(),
                    "E1114",
                    format!("implementation provides method `{name}` more than once"),
                    expected
                        .declaration_span
                        .map(|declaration| vec![("trait method declared here", declaration)]),
                    None,
                )?;
                continue;
            }
            let method_id = implementation.methods[method_index].id;
            let callable = self
                .callables
                .iter()
                .find(|callable| callable.id == HirCallableId::Implementation(method_id))
                .cloned()
                .expect("implementation methods are lowered before contract matching");
            let contract =
                self.instantiate_method_contract(implementation, &callable, expected, span)?;
            implementation.methods[method_index].contract = contract;
        }

        for expected in expected_methods {
            if !expected.has_default && !provided.contains(&expected.name) {
                self.emit(
                    implementation.span.file(),
                    implementation.span.range(),
                    "E1114",
                    format!(
                        "implementation is missing required method `{}`",
                        expected.name
                    ),
                    expected
                        .declaration_span
                        .map(|declaration| vec![("required trait method", declaration)]),
                    None,
                )?;
            }
        }
        implementation.contract_complete = self.diagnostics.len() == diagnostics_before
            && implementation
                .methods
                .iter()
                .all(|method| method.contract.is_some());
        Ok(())
    }

    fn validate_implementation_coherence(&mut self) -> Result<(), HirError> {
        let mut groups = BTreeMap::<HirTraitIdentity, Vec<usize>>::new();
        for (index, implementation) in self.implementations.iter().enumerate() {
            if !implementation.contract_complete {
                continue;
            }
            let Some(identity) = self.trait_identity(&implementation.trait_reference.constructor)
            else {
                continue;
            };
            groups.entry(identity).or_default().push(index);
        }

        let mut conflicts = Vec::new();
        for (identity, implementations) in groups {
            for left_index in 0..implementations.len() {
                for right_index in left_index + 1..implementations.len() {
                    let earlier_index = implementations[left_index];
                    let later_index = implementations[right_index];
                    let earlier = &self.implementations[earlier_index];
                    let later = &self.implementations[later_index];
                    let conflict = if matches!(
                        &identity,
                        HirTraitIdentity::Prelude(name) if name.as_str() == "Iterator"
                    ) {
                        let Some(earlier_element) =
                            earlier.trait_reference.arguments.first().copied()
                        else {
                            continue;
                        };
                        let Some(later_element) = later.trait_reference.arguments.first().copied()
                        else {
                            continue;
                        };
                        match self
                            .interner
                            .first_order_independent_equivalent_after_unifying(
                                &[earlier.target],
                                &[later.target],
                                earlier_element,
                                later_element,
                            )? {
                            None => None,
                            Some(true) => Some(CoherenceConflictKind::Overlap),
                            Some(false) => Some(CoherenceConflictKind::IteratorElement),
                        }
                    } else {
                        let mut earlier_header = earlier.trait_reference.arguments.clone();
                        earlier_header.push(earlier.target);
                        let mut later_header = later.trait_reference.arguments.clone();
                        later_header.push(later.target);
                        self.interner
                            .first_order_independent_unifiable(&earlier_header, &later_header)?
                            .then_some(CoherenceConflictKind::Overlap)
                    };
                    if let Some(conflict) = conflict {
                        conflicts.push((earlier_index, later_index, conflict));
                    }
                }
            }
        }

        for (earlier_index, later_index, conflict) in conflicts {
            let earlier = &self.implementations[earlier_index];
            let later = &self.implementations[later_index];
            let (code, message, related) = match conflict {
                CoherenceConflictKind::Overlap => (
                    "E1111",
                    "implementation header overlaps an earlier implementation",
                    "earlier overlapping implementation",
                ),
                CoherenceConflictKind::IteratorElement => (
                    "E1113",
                    "the same Iterator target can produce a different element type",
                    "earlier Iterator implementation for this target",
                ),
            };
            self.emit(
                later.span.file(),
                later.span.range(),
                code,
                message,
                Some(vec![(related, earlier.span)]),
                None,
            )?;
        }
        Ok(())
    }

    fn trait_identity(&self, constructor: &HirTraitConstructor) -> Option<HirTraitIdentity> {
        match constructor {
            HirTraitConstructor::Symbol(symbol) => self
                .resolved
                .symbol(*symbol)
                .map(|symbol| HirTraitIdentity::Symbol(symbol.identity().clone())),
            HirTraitConstructor::External(identity) => {
                Some(HirTraitIdentity::Symbol(identity.clone()))
            }
            HirTraitConstructor::Prelude(name) => Some(HirTraitIdentity::Prelude(name.clone())),
        }
    }

    fn validate_trait_termination(&mut self) -> Result<(), HirError> {
        let mut edges = Vec::new();
        for (implementation_index, implementation) in self.implementations.iter().enumerate() {
            if !implementation.contract_complete || implementation.parameters.is_empty() {
                continue;
            }
            let Some(source) = self.trait_identity(&implementation.trait_reference.constructor)
            else {
                continue;
            };
            let mut source_query = implementation.trait_reference.arguments.clone();
            source_query.push(implementation.target);
            for parameter in &implementation.parameters {
                let target = self.generic_types[&parameter.local];
                for bound in &parameter.bounds {
                    let Some(destination) = self.trait_identity(&bound.constructor) else {
                        continue;
                    };
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
        if edges.is_empty() {
            return Ok(());
        }

        let failures = match analyze_trait_termination(
            &self.interner,
            &edges,
            self.max_trait_termination_steps,
        ) {
            Ok(failures) => failures,
            Err(TraitTerminationError::ResourceLimit { .. }) => {
                let span = self.implementations[edges[0].origin].span;
                return Err(HirError::TraitObligationLimit {
                    file: span.file(),
                    offset: span.range().start(),
                });
            }
            Err(TraitTerminationError::Type(error)) => return Err(error.into()),
            Err(error) => {
                return Err(HirError::TraitTerminationInvariant {
                    message: error.to_string(),
                });
            }
        };

        let mut reports = Vec::new();
        for failure in failures {
            let primary_origin = *failure
                .origins()
                .last()
                .expect("a termination failure contains a nonempty cycle");
            let primary = self.implementations[primary_origin].span;
            let path = failure
                .traits()
                .iter()
                .map(HirTraitIdentity::canonical_name)
                .collect::<Vec<_>>()
                .join(" -> ");
            let message = format!(
                "trait obligation cycle `{path}` has idempotent size-change matrix `{}` without a decreasing diagonal",
                failure.matrix().render()
            );
            let mut seen = BTreeSet::from([primary]);
            let mut related = Vec::new();
            for origin in failure.origins() {
                let span = self.implementations[*origin].span;
                if seen.insert(span) {
                    related.push(span);
                }
            }
            reports.push((primary, message, related));
        }

        for (primary, message, related) in reports {
            self.emit(
                primary.file(),
                primary.range(),
                "E1112",
                message,
                Some(
                    related
                        .into_iter()
                        .map(|span| ("cycle obligation introduced here", span))
                        .collect(),
                ),
                None,
            )?;
        }
        Ok(())
    }

    fn validate_implementation_binders(
        &mut self,
        implementation: &HirImplementation,
    ) -> Result<(), HirError> {
        let mut positions = BTreeSet::new();
        self.collect_generic_positions(implementation.target, &mut positions)?;
        for argument in &implementation.trait_reference.arguments {
            self.collect_generic_positions(*argument, &mut positions)?;
        }
        for parameter in &implementation.parameters {
            if positions.contains(&parameter.position) {
                continue;
            }
            let span = self
                .resolved
                .local(parameter.local)
                .expect("implementation generic parameters have resolved binders")
                .span();
            self.emit(
                span.file(),
                span.range(),
                "E1114",
                "implementation binder does not appear in its trait or normalized target",
                None,
                None,
            )?;
        }
        Ok(())
    }

    fn collect_generic_positions(
        &self,
        root: TypeId,
        positions: &mut BTreeSet<u32>,
    ) -> Result<(), HirError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.interner.kind(ty)? {
                TypeKind::GenericParameter(position) => {
                    positions.insert(*position);
                }
                TypeKind::Nominal { arguments, .. }
                | TypeKind::Tuple(arguments)
                | TypeKind::Union(arguments)
                | TypeKind::Intrinsic { arguments, .. }
                | TypeKind::Generated { arguments, .. } => {
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters().iter().map(FunctionParameter::ty));
                    pending.extend(function.variadic());
                    pending.push(function.outcome());
                }
                TypeKind::Option(item) => pending.push(*item),
                TypeKind::Result { success, error } => {
                    pending.push(*success);
                    pending.push(*error);
                }
                TypeKind::Cursor { collection, .. } => pending.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(())
    }

    fn validate_orphan_rule(&mut self, implementation: &HirImplementation) -> Result<(), HirError> {
        if self.type_has_recovery(implementation.target)?
            || self.types_have_recovery(implementation.trait_reference.arguments.iter().copied())?
        {
            return Ok(());
        }
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
        let owns_target = match self.interner.kind(implementation.target)? {
            TypeKind::Nominal { identity, .. } | TypeKind::OpaqueResult(identity) => {
                identity_belongs_to(&implementation.module, identity)
            }
            TypeKind::Error
            | TypeKind::Scalar(_)
            | TypeKind::Tuple(_)
            | TypeKind::Function(_)
            | TypeKind::Option(_)
            | TypeKind::Result { .. }
            | TypeKind::Union(_)
            | TypeKind::Intrinsic { .. }
            | TypeKind::GenericParameter(_)
            | TypeKind::Inference(_)
            | TypeKind::Generated { .. }
            | TypeKind::Cursor { .. } => false,
        };
        if owns_trait || owns_target {
            return Ok(());
        }
        let related = match implementation.trait_reference.constructor {
            HirTraitConstructor::Symbol(symbol) => self
                .resolved
                .symbol(symbol)
                .map(|symbol| vec![("trait declared in another module", symbol.span())]),
            HirTraitConstructor::Prelude(_) | HirTraitConstructor::External(_) => None,
        };
        self.emit(
            implementation.span.file(),
            implementation.span.range(),
            "E1114",
            "orphan implementation: this module owns neither the trait nor the target's outer nominal constructor",
            related,
            None,
        )
    }

    fn expected_trait_methods(
        &mut self,
        implementation: &HirImplementation,
    ) -> Result<Option<Vec<ExpectedTraitMethod>>, HirError> {
        match &implementation.trait_reference.constructor {
            HirTraitConstructor::Symbol(symbol) => {
                self.expected_source_trait_methods(*symbol, implementation)
            }
            HirTraitConstructor::External(identity) => {
                if let Some(symbol) = self
                    .resolved
                    .symbols()
                    .find(|symbol| symbol.identity() == identity)
                    .map(|symbol| symbol.id())
                {
                    self.expected_source_trait_methods(symbol, implementation)
                } else {
                    self.emit(
                        implementation.span.file(),
                        implementation.span.range(),
                        "E1114",
                        "cannot validate an implementation without the imported trait contract",
                        None,
                        None,
                    )?;
                    Ok(None)
                }
            }
            HirTraitConstructor::Prelude(name) => {
                self.expected_prelude_trait_methods(name.clone(), implementation)
            }
        }
    }

    fn expected_source_trait_methods(
        &mut self,
        symbol: SymbolId,
        implementation: &HirImplementation,
    ) -> Result<Option<Vec<ExpectedTraitMethod>>, HirError> {
        let Some(declaration) = self.declarations.get(&symbol) else {
            return Ok(None);
        };
        let HirTypeDeclarationKind::Trait(definition) = &declaration.kind else {
            return Ok(None);
        };
        if implementation.trait_reference.arguments.len() != declaration.parameters.len() {
            return Ok(None);
        }
        let fixed_arity = u32::try_from(declaration.parameters.len())
            .map_err(|_| crate::types::TypeError::ResourceLimit { limit: u32::MAX })?
            .checked_add(1)
            .ok_or(crate::types::TypeError::ResourceLimit { limit: u32::MAX })?;
        let mut methods = Vec::with_capacity(definition.methods.len());
        for method in &definition.methods {
            let member = self
                .resolved
                .member(method.member)
                .expect("trait HIR methods retain resolved members");
            let Some(callable) = self
                .callables
                .iter()
                .find(|callable| callable.id == HirCallableId::Member(method.member))
                .cloned()
            else {
                self.emit(
                    implementation.span.file(),
                    implementation.span.range(),
                    "E1114",
                    format!(
                        "trait method `{}` has no available signature",
                        member.name()
                    ),
                    Some(vec![("trait method declared here", member.span())]),
                    None,
                )?;
                return Ok(None);
            };
            methods.push(ExpectedTraitMethod {
                name: Name::new(member.name().as_str())
                    .expect("resolved member names are valid names"),
                key: HirTraitMethodKey::Source(method.member),
                declaration_span: Some(member.span()),
                has_default: method.has_default,
                requires_self_send: method.requires_self_send,
                signature: ExpectedTraitMethodSignature::Source {
                    callable,
                    fixed_arity,
                },
            });
        }
        Ok(Some(methods))
    }

    fn expected_prelude_trait_methods(
        &mut self,
        name: Name,
        implementation: &HirImplementation,
    ) -> Result<Option<Vec<ExpectedTraitMethod>>, HirError> {
        let expected_arity = match name.as_str() {
            "Display" => 0,
            "Iterator" => 1,
            _ => implementation.trait_reference.arguments.len(),
        };
        if implementation.trait_reference.arguments.len() != expected_arity {
            return Ok(None);
        }
        let (method_name, key, mode, outcome) = match name.as_str() {
            "Display" => (
                "display",
                HirPreludeTraitMethod::Display,
                ParameterMode::Ref,
                self.interner.scalar(ScalarType::String),
            ),
            "Iterator" => (
                "next",
                HirPreludeTraitMethod::IteratorNext,
                ParameterMode::Mut,
                self.interner.option(
                    implementation
                        .trait_reference
                        .arguments
                        .first()
                        .copied()
                        .unwrap_or_else(|| self.interner.error()),
                )?,
            ),
            "Copy" | "Discard" | "Equatable" | "Key" | "Send" | "Share" | "Call" | "CallMut"
            | "CallOnce" => {
                self.emit(
                    implementation.span.file(),
                    implementation.span.range(),
                    "E1114",
                    format!("`{name}` is a closed protocol and cannot be implemented manually"),
                    None,
                    None,
                )?;
                return Ok(None);
            }
            _ => return Ok(None),
        };
        let function_type = self.interner.function(FunctionType::new(
            false,
            false,
            vec![FunctionParameter::new(mode, implementation.target)],
            None,
            outcome,
        ))?;
        Ok(Some(vec![ExpectedTraitMethod {
            name: Name::new(method_name).expect("prelude method names are valid"),
            key: HirTraitMethodKey::Prelude(key),
            declaration_span: None,
            has_default: false,
            requires_self_send: false,
            signature: ExpectedTraitMethodSignature::Concrete {
                function_type,
                has_receiver: true,
            },
        }]))
    }

    fn instantiate_method_contract(
        &mut self,
        implementation: &HirImplementation,
        callable: &HirCallableSignature,
        expected: &ExpectedTraitMethod,
        span: Span,
    ) -> Result<Option<HirImplementationMethodContract>, HirError> {
        let outer_arity = u32::try_from(implementation.parameters.len())
            .map_err(|_| crate::types::TypeError::ResourceLimit { limit: u32::MAX })?;
        let Some(actual_local_arity) = callable.generic_arity.checked_sub(outer_arity) else {
            self.emit(
                span.file(),
                span.range(),
                "E1114",
                format!("method `{}` has an invalid generic prefix", expected.name),
                expected
                    .declaration_span
                    .map(|declaration| vec![("trait method declared here", declaration)]),
                None,
            )?;
            return Ok(None);
        };
        let (function_type, has_receiver, generic_bounds) = match &expected.signature {
            ExpectedTraitMethodSignature::Source {
                callable: source,
                fixed_arity,
            } => {
                let Some(expected_local_arity) = source.generic_arity.checked_sub(*fixed_arity)
                else {
                    return Ok(None);
                };
                if actual_local_arity != expected_local_arity {
                    self.emit(
                        span.file(),
                        span.range(),
                        "E1114",
                        format!(
                            "method `{}` declares the wrong number of generic parameters",
                            expected.name
                        ),
                        expected
                            .declaration_span
                            .map(|declaration| vec![("trait method declared here", declaration)]),
                        Some((
                            expected_local_arity.to_string(),
                            actual_local_arity.to_string(),
                        )),
                    )?;
                    return Ok(None);
                }
                let mut arguments = implementation.trait_reference.arguments.clone();
                arguments.push(implementation.target);
                let local_end = outer_arity
                    .checked_add(actual_local_arity)
                    .ok_or(crate::types::TypeError::ResourceLimit { limit: u32::MAX })?;
                for position in outer_arity..local_end {
                    arguments.push(self.interner.generic_parameter(position)?);
                }
                let substitution = TypeSubstitution::new(arguments);
                let function_type = substitution.apply(&mut self.interner, source.function_type)?;
                let mut generic_bounds = Vec::new();
                for parameter in source
                    .generics
                    .iter()
                    .filter(|parameter| parameter.position >= *fixed_arity)
                {
                    generic_bounds.push(
                        parameter
                            .bounds
                            .iter()
                            .map(|bound| {
                                Ok(HirTraitReference {
                                    constructor: bound.constructor.clone(),
                                    arguments: bound
                                        .arguments
                                        .iter()
                                        .map(|argument| {
                                            substitution.apply(&mut self.interner, *argument)
                                        })
                                        .collect::<Result<Vec<_>, crate::types::TypeError>>()?,
                                })
                            })
                            .collect::<Result<Vec<_>, crate::types::TypeError>>()?,
                    );
                }
                (
                    function_type,
                    source.parameters.iter().any(|parameter| parameter.receiver),
                    generic_bounds,
                )
            }
            ExpectedTraitMethodSignature::Concrete {
                function_type,
                has_receiver,
            } => {
                if actual_local_arity != 0 {
                    self.emit(
                        span.file(),
                        span.range(),
                        "E1114",
                        format!(
                            "method `{}` declares the wrong number of generic parameters",
                            expected.name
                        ),
                        None,
                        Some(("0".to_owned(), actual_local_arity.to_string())),
                    )?;
                    return Ok(None);
                }
                (*function_type, *has_receiver, Vec::new())
            }
        };
        let contract = HirImplementationMethodContract {
            method: expected.key,
            has_default: expected.has_default,
            requires_self_send: expected.requires_self_send,
            function_type,
            has_receiver,
            generic_bounds,
        };
        let related = expected
            .declaration_span
            .map(|declaration| vec![("trait method declared here", declaration)]);
        if callable.function_type != contract.function_type {
            let expected_actual = self
                .interner
                .canonical(contract.function_type)
                .ok()
                .zip(self.interner.canonical(callable.function_type).ok());
            self.emit(
                span.file(),
                span.range(),
                "E1114",
                format!(
                    "method `{}` does not match the trait signature",
                    expected.name
                ),
                related.clone(),
                expected_actual,
            )?;
        }
        let actual_has_receiver = callable
            .parameters
            .iter()
            .any(|parameter| parameter.receiver);
        if actual_has_receiver != contract.has_receiver {
            self.emit(
                span.file(),
                span.range(),
                "E1114",
                format!(
                    "method `{}` must {}a receiver",
                    expected.name,
                    if contract.has_receiver {
                        "have "
                    } else {
                        "not have "
                    }
                ),
                related.clone(),
                None,
            )?;
        }
        let actual_generic_bounds = callable
            .generics
            .iter()
            .filter(|parameter| parameter.position >= outer_arity)
            .map(|parameter| parameter.bounds.clone())
            .collect::<Vec<_>>();
        if !same_generic_bound_groups(&actual_generic_bounds, &contract.generic_bounds) {
            self.emit(
                span.file(),
                span.range(),
                "E1114",
                format!(
                    "method `{}` generic bounds do not match the trait contract",
                    expected.name
                ),
                related,
                None,
            )?;
        }
        Ok(Some(contract))
    }

    fn lower_callable(
        &mut self,
        file: FileId,
        callable: SyntaxNodeRef<'a>,
        name_range: TextRange,
        id: HirCallableId,
        environment: TypeEnvironment,
        generics: Vec<HirGenericParameter>,
    ) -> Result<(), HirError> {
        let generic_arity = environment.next_position;
        let is_async = has_direct_token(callable, TokenKind::Async);
        let is_unsafe = has_direct_token(callable, TokenKind::Unsafe);
        let mut parameters = Vec::new();
        let mut function_parameters = Vec::new();
        let mut variadic = None;
        if let Some(list) = callable
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ParameterList)
        {
            let parameter_nodes = list
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::Parameter)
                .collect::<Vec<_>>();
            let mut receiver_seen = false;
            for (index, parameter) in parameter_nodes.iter().enumerate() {
                if has_direct_token(*parameter, TokenKind::SelfKw) {
                    if receiver_seen || index != 0 {
                        self.emit(
                            file,
                            parameter.range(),
                            "E1115",
                            "a receiver must be the unique first parameter",
                            None,
                            None,
                        )?;
                    }
                    receiver_seen = true;
                    let mode = if has_direct_token(*parameter, TokenKind::Mut) {
                        ParameterMode::Mut
                    } else if has_direct_token(*parameter, TokenKind::Var) {
                        ParameterMode::Var
                    } else {
                        ParameterMode::Ref
                    };
                    if is_async && matches!(mode, ParameterMode::Mut | ParameterMode::Var) {
                        self.emit(
                            file,
                            parameter.range(),
                            "E1115",
                            "an async callable cannot borrow a mutable receiver",
                            None,
                            None,
                        )?;
                    }
                    let ty = environment
                        .contextual_self
                        .unwrap_or_else(|| self.interner.error());
                    parameters.push(HirParameter {
                        span: self.sources.span(file, parameter.range())?,
                        local: None,
                        mode,
                        ty,
                        variadic_element: None,
                        receiver: true,
                        discard: false,
                    });
                    function_parameters.push(FunctionParameter::new(mode, ty));
                    continue;
                }

                let Some(ty_node) = parameter
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::TypeExpr)
                else {
                    continue;
                };
                let source_type = self.lower_type_expr(file, ty_node, &environment)?;
                let is_variadic = has_direct_token(*parameter, TokenKind::Ellipsis);
                let mode = parameter_mode(*parameter);
                if is_async && matches!(mode, ParameterMode::Mut | ParameterMode::Var) {
                    self.emit(
                        file,
                        parameter.range(),
                        "E1115",
                        "an async callable cannot borrow a mutable parameter",
                        None,
                        None,
                    )?;
                }
                let name = parameter
                    .child_tokens()
                    .find(|token| token.kind() == TokenKind::Identifier);
                let discard =
                    name.is_some_and(|name| name.token().normalized_identifier() == Some("_"));
                let local = name.and_then(|name| self.resolved.local_at(file, name.range()));
                if is_variadic {
                    if variadic.is_some() || index + 1 != parameter_nodes.len() {
                        self.emit(
                            file,
                            parameter.range(),
                            "E1115",
                            "a variadic parameter must be unique and last",
                            None,
                            None,
                        )?;
                    }
                    variadic = Some(source_type);
                    let body_type = self
                        .interner
                        .intrinsic(IntrinsicType::Array, vec![source_type])?;
                    parameters.push(HirParameter {
                        span: self.sources.span(file, parameter.range())?,
                        local: local.map(|local| local.id()),
                        mode: ParameterMode::Value,
                        ty: body_type,
                        variadic_element: Some(source_type),
                        receiver: false,
                        discard,
                    });
                } else {
                    function_parameters.push(FunctionParameter::new(mode, source_type));
                    parameters.push(HirParameter {
                        span: self.sources.span(file, parameter.range())?,
                        local: local.map(|local| local.id()),
                        mode,
                        ty: source_type,
                        variadic_element: None,
                        receiver: false,
                        discard,
                    });
                }
            }
        }

        let outcome = if let Some(annotation) = callable
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::OutcomeAnnotation)
        {
            if let Some(ty) = annotation
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
            {
                self.lower_type_expr(file, ty, &environment)?
            } else if let Some(opaque) = annotation
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::OpaqueOutcome)
            {
                self.lower_opaque_outcome(file, opaque, id, &environment)?
            } else {
                self.interner.error()
            }
        } else {
            self.interner.scalar(ScalarType::Unit)
        };
        let signature_has_recovery = self.types_have_recovery(
            function_parameters
                .iter()
                .map(FunctionParameter::ty)
                .chain(variadic)
                .chain([outcome]),
        )?;
        let function_type = if signature_has_recovery {
            self.interner.error()
        } else {
            self.interner.function(FunctionType::new(
                is_async,
                is_unsafe,
                function_parameters,
                variadic,
                outcome,
            ))?
        };
        let body = callable
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::Block);
        self.callables.push(HirCallableSignature {
            id,
            span: self.sources.span(file, name_range)?,
            parameters,
            generics,
            generic_arity,
            outcome,
            function_type,
            body_source: body
                .map(|body| self.sources.span(file, body.range()))
                .transpose()?,
        });
        if let Some(body) = body {
            self.lower_annotation_tree(file, body, &environment)?;
        }
        Ok(())
    }

    fn lower_opaque_outcome(
        &mut self,
        file: FileId,
        opaque: SyntaxNodeRef<'a>,
        callable: HirCallableId,
        environment: &TypeEnvironment,
    ) -> Result<TypeId, HirError> {
        let bounds = if let Some(bound) = opaque
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::GenericBound)
        {
            self.lower_generic_bound(file, bound, environment)?
        } else {
            Vec::new()
        };
        let discard = bounds.iter().any(|bound| match bound.constructor() {
            HirTraitConstructor::Prelude(name) => {
                matches!(name.as_str(), "Discard" | "Copy")
            }
            HirTraitConstructor::Symbol(_) | HirTraitConstructor::External(_) => false,
        });
        if !discard {
            self.emit(
                file,
                opaque.range(),
                "E1117",
                "an opaque result must prove `Discard` directly or through `Copy`",
                None,
                None,
            )?;
        }
        let identity = self.callable_identity(file, callable)?;
        let Some(identity) = identity else {
            self.emit(
                file,
                opaque.range(),
                "E1117",
                "this callable cannot own an opaque result identity",
                None,
                None,
            )?;
            return Ok(self.interner.error());
        };
        let success = self.interner.opaque_result(identity)?;
        let error_operand = opaque.child_nodes().find(|child| {
            matches!(
                child.kind(),
                SyntaxKind::OptionalType | SyntaxKind::GroupType
            )
        });
        if let Some(error) = error_operand {
            let error = self.lower_type_operand(file, error, environment)?;
            if self.type_has_recovery(error)? {
                Ok(self.interner.error())
            } else {
                Ok(self.interner.result(success, error)?)
            }
        } else {
            Ok(success)
        }
    }

    fn callable_identity(
        &self,
        file: FileId,
        callable: HirCallableId,
    ) -> Result<Option<SymbolIdentity>, HirError> {
        match callable {
            HirCallableId::Symbol(symbol) => Ok(self
                .resolved
                .symbol(symbol)
                .map(|symbol| symbol.identity().clone())),
            HirCallableId::Member(member) => {
                let Some(member) = self.resolved.member(member) else {
                    return Ok(None);
                };
                let crate::resolve::MemberOwner::Type(owner) = member.owner() else {
                    return Ok(None);
                };
                let Some(owner) = self.resolved.symbol(owner) else {
                    return Ok(None);
                };
                let module = self.packages.module_for_file(self.sources, file)?;
                Ok(Some(
                    self.packages.symbol_identity(
                        module,
                        Namespace::Value,
                        DeclarationPath::new([
                            owner.name().clone(),
                            Name::new(member.name().as_str()).expect(
                                "resolved member names are valid declaration path segments",
                            ),
                        ])
                        .expect("an owner and member form a non-empty declaration path"),
                    )?,
                ))
            }
            HirCallableId::Implementation(_) => Ok(None),
        }
    }

    fn lower_annotation_tree(
        &mut self,
        file: FileId,
        node: SyntaxNodeRef<'a>,
        environment: &TypeEnvironment,
    ) -> Result<(), HirError> {
        if node.kind() == SyntaxKind::TypeExpr {
            let _ = self.lower_type_expr(file, node, environment)?;
            return Ok(());
        }
        for child in node.child_nodes() {
            self.lower_annotation_tree(file, child, environment)?;
        }
        Ok(())
    }

    fn resolved_name_at(&self, file: FileId, token: SyntaxTokenRef<'a>) -> Option<ResolvedName> {
        match self.resolved.reference(file, token.range())?.entity() {
            ResolvedEntity::Name(name) => Some(name.clone()),
            ResolvedEntity::ContextualCandidates { type_name, .. } => Some(type_name.clone()),
            ResolvedEntity::Module(_) => None,
        }
    }

    fn validate_productivity(&mut self) -> Result<(), HirError> {
        let nominal_symbols = self
            .declarations
            .iter()
            .filter_map(|(symbol, declaration)| {
                matches!(declaration.kind, HirTypeDeclarationKind::Nominal(_)).then_some(*symbol)
            })
            .collect::<Vec<_>>();
        let by_identity = nominal_symbols
            .iter()
            .map(|symbol| {
                (
                    self.resolved
                        .symbol(*symbol)
                        .expect("HIR declarations have resolved symbols")
                        .identity()
                        .clone(),
                    *symbol,
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut adjacency = BTreeMap::new();
        for symbol in &nominal_symbols {
            let mut dependencies = BTreeSet::new();
            let declaration = &self.declarations[symbol];
            let HirTypeDeclarationKind::Nominal(nominal) = &declaration.kind else {
                continue;
            };
            for root in nominal_shape_types(&nominal.shape) {
                self.collect_nominal_dependencies(root, &by_identity, &mut dependencies)?;
            }
            adjacency.insert(*symbol, dependencies.into_iter().collect::<Vec<_>>());
        }

        for component in strongly_connected_components(&nominal_symbols, &adjacency) {
            let cyclic = component.len() > 1
                || component.first().is_some_and(|symbol| {
                    adjacency
                        .get(symbol)
                        .is_some_and(|dependencies| dependencies.contains(symbol))
                });
            if !cyclic {
                continue;
            }
            let mut productive = BTreeMap::new();
            for symbol in &component {
                let self_type = match &self.declarations[symbol].kind {
                    HirTypeDeclarationKind::Nominal(nominal) => nominal.self_type,
                    _ => unreachable!("nominal symbol selection is exact"),
                };
                let arguments = match self.interner.kind(self_type)? {
                    TypeKind::Nominal { arguments, .. } => arguments.clone(),
                    _ => unreachable!("a nominal definition has a nominal self type"),
                };
                productive.insert(
                    *symbol,
                    self.evaluate_nominal(*symbol, &arguments, &by_identity)?,
                );
            }
            if component.iter().all(|symbol| productive[symbol]) {
                continue;
            }
            let primary = component
                .iter()
                .copied()
                .find(|symbol| !productive[symbol])
                .expect("a rejected component contains a nonproductive declaration");
            let primary_symbol = self
                .resolved
                .symbol(primary)
                .expect("productivity components contain resolved symbols");
            let names = component
                .iter()
                .map(|symbol| {
                    self.resolved
                        .symbol(*symbol)
                        .expect("productivity components contain resolved symbols")
                        .name()
                        .to_string()
                })
                .collect::<Vec<_>>();
            let related = component
                .iter()
                .copied()
                .filter(|symbol| *symbol != primary)
                .map(|symbol| {
                    (
                        "type in this recursive component",
                        self.resolved
                            .symbol(symbol)
                            .expect("productivity components contain resolved symbols")
                            .span(),
                    )
                })
                .collect::<Vec<_>>();
            self.emit(
                primary_symbol.span().file(),
                primary_symbol.span().range(),
                "E1107",
                format!(
                    "recursive type component `{}` has no finite base value",
                    names.join(", ")
                ),
                Some(related),
                None,
            )?;
        }
        Ok(())
    }

    fn collect_nominal_dependencies(
        &self,
        root: TypeId,
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
        output: &mut BTreeSet<SymbolId>,
    ) -> Result<(), HirError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            match self.interner.kind(ty)? {
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => {
                    if let Some(symbol) = by_identity.get(identity) {
                        output.insert(*symbol);
                    }
                    pending.extend(arguments.iter().copied());
                }
                TypeKind::Tuple(items) | TypeKind::Union(items) => {
                    pending.extend(items.iter().copied());
                }
                TypeKind::Function(function) => {
                    pending.extend(function.parameters().iter().map(FunctionParameter::ty));
                    pending.extend(function.variadic());
                    pending.push(function.outcome());
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
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_)
                | TypeKind::OpaqueResult(_) => {}
            }
        }
        Ok(())
    }

    fn evaluate_nominal(
        &mut self,
        symbol: SymbolId,
        arguments: &[TypeId],
        by_identity: &BTreeMap<SymbolIdentity, SymbolId>,
    ) -> Result<bool, HirError> {
        let mut active = BTreeSet::new();
        let mut values = Vec::<bool>::new();
        let mut pending = vec![ProductivityTask::Nominal(symbol, arguments.to_vec())];
        while let Some(task) = pending.pop() {
            match task {
                ProductivityTask::Type(ty) => match self.interner.kind(ty)?.clone() {
                    TypeKind::Error | TypeKind::Inference(_) => values.push(true),
                    TypeKind::Scalar(scalar) => values.push(scalar != ScalarType::Never),
                    TypeKind::GenericParameter(_) => values.push(true),
                    TypeKind::Nominal {
                        identity,
                        arguments,
                    } => {
                        if let Some(symbol) = by_identity.get(&identity).copied() {
                            pending.push(ProductivityTask::Nominal(symbol, arguments));
                        } else {
                            values.push(true);
                        }
                    }
                    TypeKind::Tuple(items) => {
                        pending.push(ProductivityTask::All(items));
                    }
                    TypeKind::Function(_)
                    | TypeKind::Option(_)
                    | TypeKind::OpaqueResult(_)
                    | TypeKind::Generated { .. }
                    | TypeKind::Cursor { .. } => values.push(true),
                    TypeKind::Result { success, error } => {
                        pending.push(ProductivityTask::Any(vec![vec![success], vec![error]]));
                    }
                    TypeKind::Union(members) => {
                        pending.push(ProductivityTask::Any(
                            members.into_iter().map(|member| vec![member]).collect(),
                        ));
                    }
                    TypeKind::Intrinsic {
                        constructor,
                        arguments,
                    } => match constructor {
                        IntrinsicType::Ref => {
                            pending.push(ProductivityTask::Type(arguments[0]));
                        }
                        IntrinsicType::Array
                        | IntrinsicType::Map
                        | IntrinsicType::Set
                        | IntrinsicType::Range
                        | IntrinsicType::Pointer
                        | IntrinsicType::Join
                        | IntrinsicType::Command
                        | IntrinsicType::Pipeline
                        | IntrinsicType::NumericConversionError => values.push(true),
                    },
                },
                ProductivityTask::Nominal(symbol, arguments) => {
                    if !active.insert(symbol) {
                        values.push(false);
                        continue;
                    }
                    let Some(declaration) = self.declarations.get(&symbol) else {
                        active.remove(&symbol);
                        values.push(true);
                        continue;
                    };
                    let HirTypeDeclarationKind::Nominal(nominal) = &declaration.kind else {
                        active.remove(&symbol);
                        values.push(true);
                        continue;
                    };
                    if declaration.parameters.len() != arguments.len() {
                        active.remove(&symbol);
                        values.push(false);
                        continue;
                    }
                    let shape = nominal.shape.clone();
                    let substitution = TypeSubstitution::new(arguments);
                    pending.push(ProductivityTask::ExitNominal(symbol));
                    match shape {
                        HirNominalShape::Newtype { underlying } => {
                            pending.push(ProductivityTask::Type(
                                substitution.apply(&mut self.interner, underlying)?,
                            ));
                        }
                        HirNominalShape::Record { fields } => {
                            pending.push(ProductivityTask::All(
                                fields
                                    .into_iter()
                                    .map(|field| substitution.apply(&mut self.interner, field.ty))
                                    .collect::<Result<Vec<_>, _>>()?,
                            ));
                        }
                        HirNominalShape::Enum { variants } => {
                            let mut payloads = Vec::with_capacity(variants.len());
                            for variant in variants {
                                let types = match variant.payload {
                                    HirVariantPayload::Unit => Vec::new(),
                                    HirVariantPayload::Tuple(items) => items,
                                    HirVariantPayload::Record(fields) => {
                                        fields.into_iter().map(|field| field.ty).collect()
                                    }
                                };
                                payloads.push(
                                    types
                                        .into_iter()
                                        .map(|ty| substitution.apply(&mut self.interner, ty))
                                        .collect::<Result<Vec<_>, _>>()?,
                                );
                            }
                            pending.push(ProductivityTask::Any(payloads));
                        }
                    }
                }
                ProductivityTask::All(types) => {
                    let count = types.len();
                    pending.push(ProductivityTask::CombineAll(count));
                    for ty in types.into_iter().rev() {
                        pending.push(ProductivityTask::Type(ty));
                    }
                }
                ProductivityTask::Any(payloads) => {
                    let count = payloads.len();
                    pending.push(ProductivityTask::CombineAny(count));
                    for payload in payloads.into_iter().rev() {
                        pending.push(ProductivityTask::All(payload));
                    }
                }
                ProductivityTask::CombineAll(count) => {
                    let start = values
                        .len()
                        .checked_sub(count)
                        .expect("productivity tasks produce one value per child");
                    let result = values[start..].iter().all(|value| *value);
                    values.truncate(start);
                    values.push(result);
                }
                ProductivityTask::CombineAny(count) => {
                    let start = values
                        .len()
                        .checked_sub(count)
                        .expect("productivity tasks produce one value per variant");
                    let result = values[start..].iter().any(|value| *value);
                    values.truncate(start);
                    values.push(result);
                }
                ProductivityTask::ExitNominal(symbol) => {
                    active.remove(&symbol);
                }
            }
        }
        debug_assert!(active.is_empty());
        debug_assert_eq!(values.len(), 1);
        Ok(values.pop().unwrap_or(false))
    }

    fn emit(
        &mut self,
        file: FileId,
        range: TextRange,
        code: &str,
        message: impl Into<String>,
        related: Option<Vec<(&str, Span)>>,
        expected_actual: Option<(String, String)>,
    ) -> Result<(), HirError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(HirError::DiagnosticLimit {
                file,
                offset: range.start(),
            });
        }
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(self.sources.span(file, range)?),
        )?;
        if let Some((expected, actual)) = expected_actual {
            diagnostic = diagnostic.with_expected_actual(Some(expected), Some(actual));
        }
        for (message, span) in related.into_iter().flatten() {
            diagnostic = diagnostic.with_related(Related::new(message, span)?);
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }
}

fn first_identifier(node: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    node.descendant_tokens()
        .find(|token| token.kind() == TokenKind::Identifier)
}

fn field_name_token(node: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    let mut tokens = node
        .child_tokens()
        .filter(|token| !token.kind().is_trivia());
    let first = tokens.next()?;
    if first.kind() == TokenKind::Priv {
        let second = tokens.next();
        if second.is_some_and(|token| token.kind() != TokenKind::Colon) {
            return second;
        }
    }
    Some(first)
}

fn has_direct_token(node: SyntaxNodeRef<'_>, kind: TokenKind) -> bool {
    node.child_tokens().any(|token| token.kind() == kind)
}

fn callable_has_receiver(node: SyntaxNodeRef<'_>) -> bool {
    node.child_nodes()
        .find(|child| child.kind() == SyntaxKind::ParameterList)
        .is_some_and(|parameters| {
            parameters.child_nodes().any(|parameter| {
                parameter.kind() == SyntaxKind::Parameter
                    && has_direct_token(parameter, TokenKind::SelfKw)
            })
        })
}

fn parameter_mode(node: SyntaxNodeRef<'_>) -> ParameterMode {
    if has_direct_token(node, TokenKind::Ref) {
        ParameterMode::Ref
    } else if has_direct_token(node, TokenKind::Mut) {
        ParameterMode::Mut
    } else if has_direct_token(node, TokenKind::Var) {
        ParameterMode::Var
    } else {
        ParameterMode::Value
    }
}

fn intrinsic_type(name: &str) -> Option<IntrinsicType> {
    Some(match name {
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
        _ => return None,
    })
}

fn prelude_trait_arity(name: &str) -> Option<usize> {
    Some(match name {
        "Copy" | "Discard" | "Equatable" | "Key" | "Send" | "Share" | "Display" => 0,
        "Iterator" | "Call" | "CallMut" | "CallOnce" => 1,
        _ => return None,
    })
}

fn identity_belongs_to(module: &ModuleId, identity: &SymbolIdentity) -> bool {
    identity.package() == module.package() && identity.module() == module.path()
}

fn same_generic_bound_groups(
    left: &[Vec<HirTraitReference>],
    right: &[Vec<HirTraitReference>],
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

fn generic_group_arguments(
    file: FileId,
    groups: &[SyntaxNodeRef<'_>],
    environment: &TypeEnvironment,
    resolved: &ResolvedProgram,
) -> Vec<TypeId> {
    groups
        .iter()
        .flat_map(|group| group.child_nodes())
        .filter(|parameter| parameter.kind() == SyntaxKind::GenericParam)
        .filter_map(first_identifier)
        .filter_map(|token| resolved.local_at(file, token.range()))
        .filter_map(|local| environment.generics.get(&local.id()).copied())
        .collect()
}

fn nominal_shape_types(shape: &HirNominalShape) -> Vec<TypeId> {
    match shape {
        HirNominalShape::Newtype { underlying } => vec![*underlying],
        HirNominalShape::Record { fields } => fields.iter().map(HirField::ty).collect(),
        HirNominalShape::Enum { variants } => variants
            .iter()
            .flat_map(|variant| match variant.payload() {
                HirVariantPayload::Unit => Vec::new(),
                HirVariantPayload::Tuple(items) => items.clone(),
                HirVariantPayload::Record(fields) => {
                    fields.iter().map(HirField::ty).collect::<Vec<_>>()
                }
            })
            .collect(),
    }
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
    for neighbors in reverse.values_mut() {
        neighbors.sort_unstable();
        neighbors.dedup();
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

    use crate::package::{Edition, PackageAlias, PackageGraph, PackageId, PackageNode};
    use crate::resolve::{MemberKind, ResolvedProgram, resolve};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn lower(source: &str) -> (SourceDatabase, ResolvedProgram, HirOutput) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:hir-test").unwrap(),
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
        let output = lower_types(
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
        .unwrap();
        (sources, resolved, output)
    }

    fn lower_modules(inputs: &[(&str, &str, &str)]) -> HirOutput {
        let mut sources = SourceDatabase::new();
        let mut parsed = Vec::new();
        for (module, path, source) in inputs {
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new("source:hir-modules").unwrap(),
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
        let app = PackageId::new("pkg:hir-modules").unwrap();
        let standard = PackageId::new("pkg:std").unwrap();
        let graph = PackageGraph::new(
            app.clone(),
            standard.clone(),
            [
                PackageNode::new(
                    app,
                    SourceId::new("source:hir-modules").unwrap(),
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
        let resolution = resolve(
            &graph,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            100,
        )
        .unwrap();
        let (resolved, diagnostics) = resolution.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        lower_types(
            &graph,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap()
    }

    fn symbol(resolved: &ResolvedProgram, name: &str) -> SymbolId {
        resolved
            .symbols()
            .find(|symbol| {
                symbol.name().as_str() == name && symbol.identity().namespace() == Namespace::Type
            })
            .unwrap()
            .id()
    }

    fn codes(output: &HirOutput) -> Vec<&str> {
        output
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    type LoweringSnapshot = (Vec<String>, Vec<(String, String, String, u32)>);

    fn lowering_snapshot(inputs: &[(&str, &str)]) -> LoweringSnapshot {
        let mut sources = SourceDatabase::new();
        let mut parsed = Vec::new();
        for (path, source) in inputs {
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new("root:hir-determinism").unwrap(),
                    ModulePath::new("main").unwrap(),
                    LogicalPath::new(path).unwrap(),
                    Arc::<[u8]>::from(source.as_bytes().to_vec()),
                ))
                .unwrap();
            let lexed = lex(&sources, file, LexMode::Module).unwrap();
            assert!(lexed.diagnostics().is_empty());
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
        let root = parsed[0].0;
        let packages = PackageGraph::loose(&sources, root).unwrap();
        let resolution = resolve(
            &packages,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            100,
        )
        .unwrap();
        let (resolved, diagnostics) = resolution.into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        let output = lower_types(
            &packages,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            &resolved,
            TypeLoweringLimits {
                max_type_nodes: 100_000,
                max_trait_obligations: 100_000,
                max_diagnostics: 100,
            },
        )
        .unwrap();
        let program = output.program();
        let mut types = program
            .declarations()
            .filter_map(|(symbol, declaration)| {
                let identity = resolved.symbol(*symbol)?.identity().canonical_name();
                let ty = match declaration.kind() {
                    HirTypeDeclarationKind::Alias { target } => *target,
                    HirTypeDeclarationKind::Nominal(nominal) => nominal.self_type(),
                    HirTypeDeclarationKind::Trait(_) => return Some(format!("{identity}=trait")),
                };
                Some(format!(
                    "{identity}={}",
                    program.interner().canonical(ty).unwrap()
                ))
            })
            .chain(program.callables().map(|callable| {
                program
                    .interner()
                    .canonical(callable.function_type())
                    .unwrap()
            }))
            .chain(program.implementations().map(|implementation| {
                let source = sources.get(implementation.span().file()).unwrap();
                format!(
                    "impl#{}:{}:{}",
                    implementation.id().index(),
                    source.path(),
                    implementation
                        .methods()
                        .iter()
                        .map(|method| method.id().index().to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }))
            .collect::<Vec<_>>();
        types.sort();
        let mut diagnostics = output
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                let PrimaryLocation::Source(span) = diagnostic.location() else {
                    panic!("HIR lowering diagnostics must retain source locations")
                };
                (
                    diagnostic.code().as_str().to_owned(),
                    diagnostic.message().to_owned(),
                    sources.get(span.file()).unwrap().path().as_str().to_owned(),
                    span.range().start(),
                )
            })
            .collect::<Vec<_>>();
        diagnostics.sort();
        (types, diagnostics)
    }

    #[test]
    fn source_forms_lower_to_one_canonical_type_graph() {
        let (_, resolved, output) = lower(
            "alias Maybe[T] = Option[T]\n\
             alias Outcome[T, E] = Result[T, E]\n\
             type UserId = Int64\n\
             type Holder[T] = { value: T }\n\
             fn consume(\n\
                 value: Maybe[Int],\n\
                 callback: fn(ref (Int | String), ...Int): Result[Option[Int], String],\n\
             ): Outcome[Unit, String] {}\n",
        );
        assert!(output.diagnostics().is_empty());
        let callable = output.program().callables().next().unwrap();
        assert_eq!(
            output
                .program()
                .interner()
                .canonical(callable.function_type())
                .unwrap(),
            "fn(Int?, fn(ref (Int | String), ...Int): Int? ! String): !String"
        );
        let user = output
            .program()
            .declaration(symbol(&resolved, "UserId"))
            .unwrap();
        let HirTypeDeclarationKind::Nominal(user) = user.kind() else {
            panic!("UserId must be nominal")
        };
        let HirNominalShape::Newtype { underlying } = user.shape() else {
            panic!("UserId must be a newtype")
        };
        assert_eq!(
            output.program().interner().canonical(*underlying).unwrap(),
            "Int"
        );
        let holder = output
            .program()
            .declaration(symbol(&resolved, "Holder"))
            .unwrap();
        assert_eq!(holder.parameters()[0].position(), 0);
    }

    #[test]
    fn transparent_aliases_expand_with_complete_generic_substitution() {
        let (_, resolved, output) = lower(
            "alias Pair[T] = (T, T)\n\
             alias OptionalPair[T] = Pair[T]?\n\
             type Wrapped[T] = { value: OptionalPair[T] }\n",
        );
        assert!(output.diagnostics().is_empty());
        let declaration = output
            .program()
            .declaration(symbol(&resolved, "Wrapped"))
            .unwrap();
        let HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
            panic!("Wrapped must be nominal")
        };
        let HirNominalShape::Record { fields } = nominal.shape() else {
            panic!("Wrapped must be a record")
        };
        assert_eq!(
            output
                .program()
                .interner()
                .canonical(fields[0].ty())
                .unwrap(),
            "($0, $0)?"
        );
    }

    #[test]
    fn alias_cycles_are_rejected_once_per_component() {
        let (_, _, output) = lower(
            "alias First = Second\n\
             alias Second = Third\n\
             alias Third = First\n",
        );
        assert_eq!(codes(&output), ["E1106"]);
    }

    #[test]
    fn arity_and_trait_value_errors_are_specific() {
        let (_, _, output) = lower(
            "trait Summary {\n\
                 fn summarize(self): String\n\
             }\n\
             fn invalid(first: Array[Int, String], second: Summary) {}\n",
        );
        assert_eq!(codes(&output), ["E1104", "E1110"]);

        let (_, _, implementation) = lower(
            "trait Generic[T] {\n\
                 fn use(self, value: T)\n\
             }\n\
             type Value = Int\n\
             impl Generic for Value {\n\
                 fn use(self, value: Int) {}\n\
             }\n",
        );
        assert_eq!(codes(&implementation), ["E1104"]);
        assert!(
            !implementation
                .program()
                .implementations()
                .next()
                .unwrap()
                .contract_complete()
        );
    }

    #[test]
    fn unions_require_discriminable_nonoverlapping_members() {
        let (_, _, output) = lower(
            "fn structural(value: (Int, String) | String) {}\n\
             fn overlapping[T](value: Array[T] | Array[Int]) {}\n",
        );
        assert_eq!(codes(&output), ["E1115", "E1115"]);
    }

    #[test]
    fn inherent_generics_receivers_and_variadics_keep_their_semantics() {
        let (_, _, output) = lower(
            "type Pair[A, B] = {\n\
                 first: A\n\
                 second: B\n\
             }\n\
             fn Pair[A, B].combine[U](self, other: U, parts: ...String): Pair[B, A] {\n\
                 self\n\
             }\n",
        );
        assert!(output.diagnostics().is_empty());
        let callable = output.program().callables().next().unwrap();
        assert_eq!(callable.generics().len(), 3);
        assert!(callable.parameters()[0].is_receiver());
        assert_eq!(callable.parameters()[0].mode(), ParameterMode::Ref);
        assert!(callable.parameters()[2].variadic_element().is_some());
        assert!(matches!(
            output
                .program()
                .interner()
                .kind(callable.parameters()[2].ty())
                .unwrap(),
            TypeKind::Intrinsic {
                constructor: IntrinsicType::Array,
                ..
            }
        ));
    }

    #[test]
    fn recursive_productivity_accepts_real_bases_and_rejects_immediate_cycles() {
        let (_, _, output) = lower(
            "enum Json {\n\
                 Null\n\
                 Children(Array[Json])\n\
             }\n\
             type Chain = (Int, Chain?)\n\
             type Holder[T] = { value: T }\n\
             type Invalid = Holder[Invalid]\n\
             type First = { second: Second }\n\
             type Second = { first: First }\n",
        );
        assert_eq!(codes(&output), ["E1107", "E1107"]);
    }

    #[test]
    fn mutual_recursion_is_productive_when_one_path_has_a_base_variant() {
        let (_, _, output) = lower(
            "type Left = Right\n\
             enum Right {\n\
                 Base\n\
                 Again(Left)\n\
             }\n",
        );
        assert!(output.diagnostics().is_empty());
    }

    #[test]
    fn productivity_substitutes_generic_arguments_before_finding_a_base() {
        let (_, _, output) = lower(
            "enum Container[T] {\n\
                 Value(T)\n\
                 Again(Invalid)\n\
             }\n\
             type Invalid = Container[Never]\n",
        );
        assert_eq!(codes(&output), ["E1107"]);
    }

    #[test]
    fn generic_bounds_are_normalized_and_must_name_traits() {
        let (_, resolved, output) = lower(
            "trait Render {}\n\
             type Value = Int\n\
             fn valid[T: Render + Copy](value: T): T { value }\n",
        );
        assert!(output.diagnostics().is_empty());
        let callable = output.program().callables().next().unwrap();
        assert_eq!(callable.generics()[0].bounds().len(), 2);
        assert_eq!(
            resolved.symbol(symbol(&resolved, "Render")).unwrap().kind(),
            SymbolKind::Trait
        );

        let (_, _, invalid) = lower(
            "type Value = Int\n\
             fn invalid[T: Value](value: T): T { value }\n",
        );
        assert_eq!(codes(&invalid), ["E1115"]);
    }

    #[test]
    fn self_is_available_before_trait_inherent_and_impl_bounds_are_lowered() {
        let (_, _, output) = lower(
            "trait Marker {\n\
                 fn marker(self)\n\
             }\n\
             trait Convert[T: Iterator[Self]] {\n\
                 fn convert[U](self, value: U): T\n\
             }\n\
             type Box[T] = { value: T }\n\
             fn Box[T: Iterator[Self]].inspect(self) {}\n\
             impl[T: Iterator[Self]] Marker for Box[T] {\n\
                 fn marker(self) {}\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        let convert = output
            .program()
            .callables()
            .find(|callable| callable.generics().len() == 2)
            .unwrap();
        assert_eq!(
            convert
                .generics()
                .iter()
                .map(HirGenericParameter::position)
                .collect::<Vec<_>>(),
            [0, 2]
        );
    }

    #[test]
    fn traits_materialize_contextual_self_defaults_and_async_requirements() {
        let (_, resolved, output) = lower(
            "trait Catalog[T: Discard] {\n\
                 fn required(self, other: ref Self): T\n\
                 fn create[U](value: U): Self\n\
                 fn defaulted[U](self, value: U): U { value }\n\
                 async fn poll(self): Bool { true }\n\
                 async fn version(): Int { 1 }\n\
             }\n\
             trait Empty {}\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );

        let catalog = output
            .program()
            .declaration(symbol(&resolved, "Catalog"))
            .unwrap();
        let HirTypeDeclarationKind::Trait(definition) = catalog.kind() else {
            panic!("Catalog must lower as a trait")
        };
        assert_eq!(
            output
                .program()
                .interner()
                .canonical(definition.self_type())
                .unwrap(),
            "$1"
        );
        assert!(
            definition
                .methods()
                .windows(2)
                .all(|methods| methods[0].member() < methods[1].member())
        );

        let methods = definition
            .methods()
            .iter()
            .map(|method| {
                let member = resolved.member(method.member()).unwrap();
                (
                    member.name().as_str(),
                    member.kind(),
                    method.has_default(),
                    method.requires_self_send(),
                    output
                        .program()
                        .callable(HirCallableId::Member(method.member()))
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let method = |name: &str| methods.iter().find(|method| method.0 == name).unwrap();

        assert_eq!(method("required").1, MemberKind::TraitMethod);
        assert!(!method("required").2);
        assert!(!method("required").3);
        assert_eq!(method("required").4.generic_arity(), 2);
        assert_eq!(method("create").1, MemberKind::TraitAssociatedFunction);
        assert!(!method("create").2);
        assert_eq!(method("create").4.generic_arity(), 3);
        assert_eq!(method("defaulted").1, MemberKind::TraitMethod);
        assert!(method("defaulted").2);
        assert_eq!(method("defaulted").4.generic_arity(), 3);
        assert_eq!(
            method("defaulted")
                .4
                .generics()
                .iter()
                .map(HirGenericParameter::position)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert!(method("poll").2);
        assert!(method("poll").3);
        assert!(method("version").2);
        assert!(!method("version").3);

        let empty = output
            .program()
            .declaration(symbol(&resolved, "Empty"))
            .unwrap();
        let HirTypeDeclarationKind::Trait(empty) = empty.kind() else {
            panic!("Empty must lower as a trait")
        };
        assert!(empty.methods().is_empty());
        assert_eq!(
            output
                .program()
                .interner()
                .canonical(empty.self_type())
                .unwrap(),
            "$0"
        );
    }

    #[test]
    fn implementations_materialize_exact_source_and_prelude_contracts() {
        let (_, resolved, output) = lower(
            "trait Codec[T] {\n\
                 fn encode[U: Display](self, value: U): T\n\
                 fn fallback(self): Bool { true }\n\
                 fn create(value: T): Self\n\
             }\n\
             type Box[T] = { value: T }\n\
             impl[T] Codec[T] for Box[T] {\n\
                 fn encode[U: Display](self, value: U): T { panic(\"todo\") }\n\
                 fn fallback(self): Bool { false }\n\
                 fn create(value: T): Self { panic(\"todo\") }\n\
             }\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        let implementation = output.program().implementations().next().unwrap();
        assert_eq!(implementation.id().index(), 0);
        assert!(implementation.contract_complete());
        assert!(!implementation.requires_self_send());
        assert_eq!(implementation.parameters().len(), 1);
        assert_eq!(implementation.trait_reference().arguments().len(), 1);
        assert_eq!(implementation.methods().len(), 3);
        for (index, method) in implementation.methods().iter().enumerate() {
            assert_eq!(method.id().implementation(), implementation.id());
            assert_eq!(method.id().index(), index as u32);
            let contract = method.contract().unwrap();
            let callable = output
                .program()
                .callable(HirCallableId::Implementation(method.id()))
                .unwrap();
            assert_eq!(callable.function_type(), contract.function_type());
            assert!(callable.body_source().is_some());
        }
        let encode = implementation
            .methods()
            .iter()
            .find(|method| method.name().as_str() == "encode")
            .unwrap();
        assert_eq!(encode.contract().unwrap().generic_bounds().len(), 1);
        assert_eq!(encode.contract().unwrap().generic_bounds()[0].len(), 1);
        let fallback = implementation
            .methods()
            .iter()
            .find(|method| method.name().as_str() == "fallback")
            .unwrap();
        assert!(fallback.contract().unwrap().has_default());
        let codec = symbol(&resolved, "Codec");
        assert!(implementation.methods().iter().all(|method| {
            let HirTraitMethodKey::Source(member) = method.contract().unwrap().method() else {
                return false;
            };
            resolved.member(member).unwrap().owner() == crate::resolve::MemberOwner::Type(codec)
        }));

        let (_, _, prelude) = lower(
            "type Label = String\n\
             type Counter = Int\n\
             impl Display for Label {\n\
                 fn display(self): String { \"label\" }\n\
             }\n\
             impl Iterator[Int] for Counter {\n\
                 fn next(mut self): Int? { none }\n\
             }\n",
        );
        assert!(
            prelude.diagnostics().is_empty(),
            "{:#?}",
            prelude.diagnostics()
        );
        let implementations = prelude.program().implementations().collect::<Vec<_>>();
        assert_eq!(implementations.len(), 2);
        assert_eq!(
            implementations[0].methods()[0].contract().unwrap().method(),
            HirTraitMethodKey::Prelude(HirPreludeTraitMethod::Display)
        );
        assert_eq!(
            implementations[1].methods()[0].contract().unwrap().method(),
            HirTraitMethodKey::Prelude(HirPreludeTraitMethod::IteratorNext)
        );
    }

    #[test]
    fn implementations_accept_omitted_defaults_and_reject_contract_drift() {
        let (_, _, omitted) = lower(
            "trait Contract {\n\
                 fn required(self): Int\n\
                 fn defaulted(self): Bool { true }\n\
             }\n\
             type Item = Int\n\
             impl Contract for Item {\n\
                 fn required(self): Int { 1 }\n\
             }\n",
        );
        assert!(
            omitted.diagnostics().is_empty(),
            "{:#?}",
            omitted.diagnostics()
        );
        assert!(
            omitted
                .program()
                .implementations()
                .next()
                .unwrap()
                .contract_complete()
        );

        for source in [
            "trait Contract {\n\
                 fn required(self): Int\n\
             }\n\
             type Item = Int\n\
             impl Contract for Item {\n\
             }\n",
            "trait Contract {}\n\
             type Item = Int\n\
             impl Contract for Item {\n\
                 fn extra(self) {}\n\
             }\n",
            "trait Contract {\n\
                 fn run(self): Int\n\
             }\n\
             type Item = Int\n\
             impl Contract for Item {\n\
                 fn run(value: Item): String { \"bad\" }\n\
             }\n",
            "trait Contract {\n\
                 fn map[U: Display](self, value: U): U\n\
             }\n\
             type Item = Int\n\
             impl Contract for Item {\n\
                 fn map[U: Discard](self, value: U): U { value }\n\
             }\n",
            "trait Contract {\n\
                 async fn run(self): Int\n\
             }\n\
             type Item = Int\n\
             impl Contract for Item {\n\
                 fn run(self): Int { 1 }\n\
             }\n",
        ] {
            let (_, _, invalid) = lower(source);
            assert!(
                !invalid.diagnostics().is_empty()
                    && invalid
                        .diagnostics()
                        .iter()
                        .all(|diagnostic| diagnostic.code().as_str() == "E1114"),
                "{source}\n{:#?}",
                invalid.diagnostics()
            );
            assert!(
                !invalid
                    .program()
                    .implementations()
                    .next()
                    .unwrap()
                    .contract_complete()
            );
        }
    }

    #[test]
    fn implementation_binders_closed_protocols_and_orphan_rules_are_enforced() {
        let (_, _, unused) = lower(
            "type Value = Int\n\
             impl[T: Discard] Display for Value {\n\
                 fn display(self): String { \"value\" }\n\
             }\n",
        );
        assert_eq!(codes(&unused), ["E1114"]);
        assert!(unused.diagnostics()[0].message().contains("binder"));

        let (_, _, closed) = lower(
            "type Value = Int\n\
             impl Copy for Value {\n\
                 fn copy(self): Value { self }\n\
             }\n",
        );
        assert_eq!(codes(&closed), ["E1114"]);
        assert!(
            closed.diagnostics()[0]
                .message()
                .contains("closed protocol")
        );

        let (_, _, owned_trait) = lower(
            "trait Local {\n\
                 fn inspect(self)\n\
             }\n\
             impl Local for Array[Int] {\n\
                 fn inspect(self) {}\n\
             }\n",
        );
        assert!(
            owned_trait.diagnostics().is_empty(),
            "{:#?}",
            owned_trait.diagnostics()
        );

        let modules = lower_modules(&[
            (
                "api",
                "api.to",
                "pub trait ForeignTrait {\n\
                     fn apply(self)\n\
                 }\n\
                 pub type ForeignType = Int\n",
            ),
            (
                "main",
                "main.to",
                "import app.api\n\
                 type LocalType = Int\n\
                 impl api.ForeignTrait for LocalType {\n\
                     fn apply(self) {}\n\
                 }\n\
                 impl api.ForeignTrait for api.ForeignType {\n\
                     fn apply(self) {}\n\
                 }\n",
            ),
        ]);
        assert_eq!(codes(&modules), ["E1114"]);
        assert!(modules.diagnostics()[0].message().contains("orphan"));
        let implementations = modules.program().implementations().collect::<Vec<_>>();
        assert_eq!(implementations.len(), 2);
        assert!(implementations[0].contract_complete());
        assert!(!implementations[1].contract_complete());
    }

    #[test]
    fn implementation_ids_follow_logical_source_order() {
        let contract = "trait Show {\n\
                            fn show(self): String\n\
                        }\n\
                        type Alpha = Int\n\
                        type Zeta = Int\n";
        let alpha = "impl Show for Alpha {\n    fn show(self): String { \"a\" }\n}\n";
        let zeta = "impl Show for Zeta {\n    fn show(self): String { \"z\" }\n}\n";
        let forward = lowering_snapshot(&[
            ("contract.to", contract),
            ("z_impl.to", zeta),
            ("a_impl.to", alpha),
        ]);
        let reverse = lowering_snapshot(&[
            ("a_impl.to", alpha),
            ("z_impl.to", zeta),
            ("contract.to", contract),
        ]);
        assert_eq!(forward, reverse);
        assert!(forward.0.iter().any(|item| item == "impl#0:a_impl.to:0"));
        assert!(forward.0.iter().any(|item| item == "impl#1:z_impl.to:0"));
    }

    #[test]
    fn coherence_diagnostics_follow_logical_source_order() {
        let contract = "trait Marker {}\n\
                        type Box[T] = { value: T }\n";
        let broad = "impl[T] Marker for Box[T] {}\n";
        let nested = "impl[U] Marker for Box[Array[U]] {}\n";
        let forward = lowering_snapshot(&[
            ("contract.to", contract),
            ("z_impl.to", nested),
            ("a_impl.to", broad),
        ]);
        let reverse = lowering_snapshot(&[
            ("a_impl.to", broad),
            ("z_impl.to", nested),
            ("contract.to", contract),
        ]);
        assert_eq!(forward, reverse);
        assert_eq!(forward.1.len(), 1);
        assert_eq!(forward.1[0].0, "E1111");
        assert_eq!(forward.1[0].2, "z_impl.to");
    }

    #[test]
    fn coherence_rejects_generic_overlap_and_ignores_positive_bounds() {
        let (_, _, independently_scoped) = lower(
            "trait Marker {}\n\
             type Box[T] = { value: T }\n\
             impl[T] Marker for Box[T] {}\n\
             impl[U] Marker for Box[Array[U]] {}\n",
        );
        assert_eq!(codes(&independently_scoped), ["E1111"]);
        assert!(
            independently_scoped.diagnostics()[0]
                .message()
                .contains("overlaps")
        );

        let (_, _, bounded) = lower(
            "trait Left {}\n\
             trait Right {}\n\
             trait Marker {}\n\
             type Box[T] = { value: T }\n\
             impl[T: Left] Marker for Box[T] {}\n\
             impl[U: Right] Marker for Box[U] {}\n",
        );
        assert_eq!(codes(&bounded), ["E1111"]);

        let (_, _, aliases) = lower(
            "trait Marker {}\n\
             type Box[T] = { value: T }\n\
             alias Wrapped[T] = Box[T]\n\
             impl Marker for Box[Int] {}\n\
             impl Marker for Wrapped[Int] {}\n",
        );
        assert_eq!(codes(&aliases), ["E1111"]);
    }

    #[test]
    fn coherence_keeps_nonunifiable_trait_instantiations_distinct() {
        let (_, _, output) = lower(
            "trait Codec[T] {}\n\
             trait Other {}\n\
             type Json = { id: Int }\n\
             type Xml = { id: Int }\n\
             type Payload = { value: Int }\n\
             impl Codec[Json] for Payload {}\n\
             impl Codec[Xml] for Payload {}\n\
             impl Other for Payload {}\n",
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );

        let (_, _, invalid_contract) = lower(
            "trait Marker {\n\
                 fn mark(self)\n\
             }\n\
             type Payload = { value: Int }\n\
             impl Marker for Payload {\n\
                 fn mark(self) {}\n\
             }\n\
             impl Marker for Payload {}\n",
        );
        assert_eq!(codes(&invalid_contract), ["E1114"]);
    }

    #[test]
    fn iterator_coherence_distinguishes_overlap_from_element_conflicts() {
        let (_, _, conflicting_element) = lower(
            "type Cursor = { value: Int }\n\
             impl Iterator[Int] for Cursor {\n\
                 fn next(mut self): Int? { none }\n\
             }\n\
             impl Iterator[String] for Cursor {\n\
                 fn next(mut self): String? { none }\n\
             }\n",
        );
        assert_eq!(codes(&conflicting_element), ["E1113"]);

        let (_, _, overlapping) = lower(
            "type Cursor[T] = { value: T }\n\
             impl[T] Iterator[T] for Cursor[T] {\n\
                 fn next(mut self): T? { none }\n\
             }\n\
             impl Iterator[String] for Cursor[String] {\n\
                 fn next(mut self): String? { none }\n\
             }\n",
        );
        assert_eq!(codes(&overlapping), ["E1111"]);

        let (_, _, substituted_conflict) = lower(
            "type Cursor[T] = { value: T }\n\
             impl[T] Iterator[T] for Cursor[T] {\n\
                 fn next(mut self): T? { none }\n\
             }\n\
             impl Iterator[Int] for Cursor[String] {\n\
                 fn next(mut self): Int? { none }\n\
             }\n",
        );
        assert_eq!(codes(&substituted_conflict), ["E1113"]);
    }

    #[test]
    fn trait_termination_accepts_structural_descent_and_acyclic_adapters() {
        let (_, _, descending) = lower(
            "trait Walk {}\n\
             impl[T: Walk] Walk for Array[T] {}\n",
        );
        assert!(
            descending.diagnostics().is_empty(),
            "{:#?}",
            descending.diagnostics()
        );

        let (_, _, mutual_descent) = lower(
            "trait Left {}\n\
             trait Right {}\n\
             impl[T: Right] Left for Array[T] {}\n\
             impl[T: Left] Right for T {}\n",
        );
        assert!(
            mutual_descent.diagnostics().is_empty(),
            "{:#?}",
            mutual_descent.diagnostics()
        );

        let (_, _, acyclic) = lower(
            "trait Summary {}\n\
             trait Render {}\n\
             impl[T: Summary] Render for T {}\n",
        );
        assert!(
            acyclic.diagnostics().is_empty(),
            "{:#?}",
            acyclic.diagnostics()
        );

        let (_, _, closed_bound) = lower(
            "trait Render {}\n\
             type Box[T] = { value: T }\n\
             impl[T: Discard] Render for Box[T] {}\n",
        );
        assert!(
            closed_bound.diagnostics().is_empty(),
            "{:#?}",
            closed_bound.diagnostics()
        );
    }

    #[test]
    fn trait_termination_rejects_equal_permuting_and_growing_cycles() {
        for source in [
            "trait Loop {}\n\
             impl[T: Loop] Loop for T {}\n",
            "trait Left {}\n\
             trait Right {}\n\
             impl[T: Right] Left for T {}\n\
             impl[T: Left] Right for T {}\n",
            "trait Rotate[A, B] {}\n\
             impl[A: Rotate[B, A], B] Rotate[A, B] for A {}\n",
            "trait Grow[T] {}\n\
             impl[T: Grow[Array[T]]] Grow[T] for T {}\n",
        ] {
            let (_, _, output) = lower(source);
            assert_eq!(
                codes(&output),
                ["E1112"],
                "{source}\n{:#?}",
                output.diagnostics()
            );
            let diagnostic = &output.diagnostics()[0];
            assert!(diagnostic.message().contains("cycle"));
            assert!(diagnostic.message().contains("matrix"));
            assert!(
                diagnostic
                    .message()
                    .contains("without a decreasing diagonal")
            );
        }
    }

    #[test]
    fn overlap_preempts_trait_termination_diagnostics() {
        let (_, _, output) = lower(
            "trait Loop {}\n\
             impl[T: Loop] Loop for T {}\n\
             impl[U: Loop] Loop for Array[U] {}\n",
        );
        assert_eq!(codes(&output), ["E1111"]);
    }

    #[test]
    fn trait_termination_witnesses_follow_logical_source_order() {
        let contract = "trait Left {}\ntrait Right {}\n";
        let left = "impl[T: Right] Left for T {}\n";
        let right = "impl[T: Left] Right for T {}\n";
        let forward = lowering_snapshot(&[
            ("contract.to", contract),
            ("z_left.to", left),
            ("a_right.to", right),
        ]);
        let reverse = lowering_snapshot(&[
            ("a_right.to", right),
            ("z_left.to", left),
            ("contract.to", contract),
        ]);
        assert_eq!(forward, reverse);
        assert_eq!(forward.1.len(), 1);
        assert_eq!(forward.1[0].0, "E1112");
        assert_eq!(forward.1[0].2, "a_right.to");
    }

    #[test]
    fn an_invalid_nested_argument_recovers_without_interning_partial_types() {
        let (_, _, output) = lower("fn invalid(value: Option[Array[Int, String]] | String) {}\n");
        assert_eq!(codes(&output), ["E1104"]);
        let callable = output.program().callables().next().unwrap();
        assert_eq!(
            callable.function_type(),
            output.program().interner().error()
        );
    }

    #[test]
    fn opaque_results_have_declaration_identity_and_require_discard() {
        let (_, _, output) = lower(
            "fn valid(): impl Iterator[Int] + Discard { panic(\"todo\") }\n\
             fn invalid(): impl Iterator[Int] { panic(\"todo\") }\n",
        );
        assert_eq!(codes(&output), ["E1117"]);
        let mut callables = output.program().callables();
        let valid = callables.next().unwrap();
        assert!(matches!(
            output.program().interner().kind(valid.outcome()).unwrap(),
            TypeKind::OpaqueResult(_)
        ));
    }

    #[test]
    fn lowering_is_independent_from_file_insertion_order() {
        let declarations = "alias Pair[T] = (T, T)\n\
                            type Node[T] = { value: T, next: Node[T]? }\n";
        let use_site = "fn consume(value: Pair[Node[Int]]): Result[Unit, String] {}\n";
        let forward = lowering_snapshot(&[("declarations.to", declarations), ("use.to", use_site)]);
        let reverse = lowering_snapshot(&[("use.to", use_site), ("declarations.to", declarations)]);
        assert_eq!(forward, reverse);
    }

    #[test]
    fn productivity_uses_an_explicit_worklist_for_deep_nominal_graphs() {
        let depth = 2_048;
        let mut source = String::new();
        for index in 0..depth - 1 {
            source.push_str(&format!(
                "type Step{index}[T] = {{ next: Step{}[T] }}\n",
                index + 1
            ));
        }
        source.push_str(&format!("type Step{}[T] = {{ next: T }}\n", depth - 1));
        source.push_str("type Invalid = Step0[Invalid]\n");
        let (_, _, output) = lower(&source);
        assert_eq!(codes(&output), ["E1107"]);
    }
}
