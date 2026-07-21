use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{DeclarationPath, Name, Namespace, PackageGraph, SymbolIdentity};
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

use super::{
    HirCallableId, HirCallableSignature, HirConstant, HirError, HirField, HirGenericParameter,
    HirNominalDefinition, HirNominalShape, HirOutput, HirParameter, HirProgram,
    HirTraitConstructor, HirTraitDefinition, HirTraitMethod, HirTraitReference, HirTypeDeclaration,
    HirTypeDeclarationKind, HirVariant, HirVariantPayload,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeLoweringLimits {
    pub max_type_nodes: u32,
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
        annotations: BTreeMap::new(),
        generic_types: BTreeMap::new(),
    };
    lowerer.index_declarations()?;
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
                    SyntaxKind::ImplDecl => self.lower_implementation(file, node)?,
                    SyntaxKind::TypeDecl | SyntaxKind::AliasDecl | SyntaxKind::EnumDecl => {}
                    SyntaxKind::ImportDecl => {}
                    _ => self.lower_annotation_tree(file, node, &TypeEnvironment::default())?,
                }
            }
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
        file: FileId,
        declaration: SyntaxNodeRef<'a>,
    ) -> Result<(), HirError> {
        let groups = declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
            .collect::<Vec<_>>();
        let mut environment = TypeEnvironment::default();
        let (generic_declarations, mut outer_parameters) =
            self.declare_generics(file, &groups, &mut environment)?;
        if let Some(target) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypeExpr)
        {
            environment.contextual_self = Some(self.lower_type_expr(file, target, &environment)?);
        }
        if let Some(trait_path) = declaration
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypePath)
        {
            let _ = self.lower_trait_path(file, trait_path, &environment)?;
        }
        self.finish_generic_bounds(
            file,
            &generic_declarations,
            &mut outer_parameters,
            &environment,
        )?;
        for method in declaration
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::ImplementationMethod)
        {
            let Some(method_name) = first_identifier(method) else {
                continue;
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
                HirCallableId::Implementation(self.sources.span(file, method_name.range())?),
                method_environment,
                generics,
            )?;
        }
        Ok(())
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

    use crate::package::PackageGraph;
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
                max_diagnostics: 100,
            },
        )
        .unwrap();
        (sources, resolved, output)
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

    fn lowering_snapshot(inputs: &[(&str, &str)]) -> (Vec<String>, Vec<(String, String)>) {
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
            .collect::<Vec<_>>();
        types.sort();
        let mut diagnostics = output
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.code().as_str().to_owned(),
                    diagnostic.message().to_owned(),
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
            "trait Marker {}\n\
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
