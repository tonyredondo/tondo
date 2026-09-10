//! Production integration between typed `derive` requests and hermetic meta providers.
//!
//! The first frontend pass authorizes a nominal target and constructs a sealed
//! snapshot. Providers emit ordinary Tondo source, which the driver compiles in
//! one final pass. No provider can observe ambient state or trigger another
//! generation round.

use std::collections::BTreeSet;

use crate::hir::{
    HirNominalShape, HirProgram, HirTypeDeclaration, HirTypeDeclarationKind, HirVariantPayload,
};
use crate::meta::{
    DeriveContext, DeriveProvider, DeriveRequest, DeriveTarget, DeriveTargetKind, MetaAttribute,
    MetaDeclaration, MetaDeclarationKind, MetaDiagnosticCode, MetaField, MetaGenericParameter,
    MetaLimits, MetaRoot, MetaVariant, MetaVariantPayload, MetaVisibility,
    validate_derive_requests,
};
use crate::meta_derive::DeriveProviderRegistry;
use crate::meta_diagnostics::{MetaDiagnosticEntry, derive_execution_entry, semantic_entry};
use crate::package::PackageGraph;
use crate::resolve::{ResolvedProgram, Visibility};
use crate::serialization_derive::{
    DECODE_PROVIDER, DECODE_TRAIT, ENCODE_PROVIDER, ENCODE_TRAIT, register_serialization_providers,
};
use crate::source::{
    FileId, ModulePath, SourceDatabase, SourceDiagnosticMapping, SourceId, Span, TextRange,
};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, TokenKind};

#[path = "meta_derive_provenance.rs"]
mod provenance;

#[derive(Debug)]
pub(crate) struct GeneratedDeriveSource {
    pub owner: crate::package::PackageId,
    pub source_id: SourceId,
    pub module: ModulePath,
    pub path: String,
    pub bytes: Vec<u8>,
    pub diagnostic_mappings: Vec<SourceDiagnosticMapping>,
    pub baseline_header: String,
    pub request_span: Span,
}

#[derive(Debug, Default)]
pub(crate) struct DeriveExpansion {
    pub sources: Vec<GeneratedDeriveSource>,
    pub diagnostics: Vec<MetaDiagnosticEntry>,
    pub accepted: Vec<crate::meta_atomic::AcceptedMetaResult>,
    pub descriptors: Vec<crate::meta_query::MetaQueryDescriptor>,
}

#[derive(Debug)]
pub(crate) enum DeriveFrontendError {
    Diagnostics(Vec<MetaDiagnosticEntry>),
    Invariant(String),
}

pub(crate) struct DeriveSettings<'a> {
    pub limits: MetaLimits,
    pub source_providers: &'a crate::meta_provider::SourceDeriveProviders,
    pub environment: &'a crate::meta::MetaEnvironment,
}

pub(crate) fn expand_derives(
    packages: &PackageGraph,
    sources: &SourceDatabase,
    parsed: &[(FileId, Parsed)],
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    settings: DeriveSettings<'_>,
) -> Result<DeriveExpansion, DeriveFrontendError> {
    let DeriveSettings {
        limits,
        source_providers,
        environment,
    } = settings;
    if hir.derive_requests().is_empty() {
        return Ok(DeriveExpansion::default());
    }

    let mut registry = DeriveProviderRegistry::default();
    register_serialization_providers(&mut registry)
        .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?;
    let mut seen = BTreeSet::new();
    let mut registered_sources = BTreeSet::new();
    let mut generated = Vec::new();
    let mut messages = Vec::new();
    let mut diagnostics = Vec::new();
    let mut accepted = Vec::new();
    let mut descriptors = Vec::new();
    let standard = crate::project_meta::bootstrap_meta_descriptor().map_err(model_error)?;

    for hir_request in hir.derive_requests() {
        if !parsed
            .iter()
            .any(|(file, _)| *file == hir_request.span().file())
        {
            continue;
        }
        let module = packages
            .module_for_file(sources, hir_request.span().file())
            .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?;
        let module_name = module.path().as_str();
        let target_name = hir_request
            .target()
            .rsplit('.')
            .next()
            .unwrap_or(hir_request.target());
        let target = resolved.symbols().find(|symbol| {
            symbol.name().as_str() == target_name
                && symbol.identity().module() == module.path()
                && symbol.identity().package() == module.package()
                && hir.declaration(symbol.id()).is_some()
        });

        let Some(target) = target else {
            let request = DeriveRequest::from_hir(module_name, hir_request);
            let context = DeriveContext::new(module_name);
            if let Err(error) = validate_derive_requests(&[request], &context) {
                diagnostics.extend(error.diagnostics().iter().map(semantic_entry));
            }
            continue;
        };
        let declaration = hir
            .declaration(target.id())
            .expect("the target predicate retained its HIR declaration");
        let target_kind = derive_target_kind(declaration);
        let syntax = declaration_syntax(parsed, declaration.span()).ok_or_else(|| {
            DeriveFrontendError::Invariant(format!(
                "derive target `{target_name}` has no source declaration"
            ))
        })?;
        let meta_declaration = build_meta_declaration(
            packages,
            module.package(),
            sources,
            resolved,
            hir,
            target,
            declaration,
            syntax,
            module_name,
        )?;
        let generic_names = meta_declaration
            .generic_parameters()
            .iter()
            .map(|parameter| parameter.name().to_owned())
            .collect::<Vec<_>>();

        let mut context = DeriveContext::new(module_name);
        context.add_target(DeriveTarget::new(
            target_name,
            module_name,
            generic_names.iter().cloned(),
            target_kind,
        ));
        let mut request_limits = std::collections::BTreeMap::new();
        let mut provider_locks = std::collections::BTreeMap::new();
        for (trait_index, trait_identity) in hir_request.traits().iter().enumerate() {
            let standard_provider = standard_trait_provider(
                packages,
                resolved,
                hir_request.span().file(),
                trait_identity,
            );
            let trait_symbol =
                requested_trait_symbol(parsed, resolved, hir, hir_request, trait_index);
            let canonical_trait = trait_symbol
                .map(|symbol| symbol.identity().canonical_name())
                .unwrap_or_else(|| {
                    standard_provider.map_or_else(
                        || trait_identity.clone(),
                        |(canonical, _)| format!("{}::{canonical}", packages.standard()),
                    )
                });
            let arguments = hir_request.trait_arguments()[trait_index]
                .iter()
                .map(|ty| hir.interner().canonical_interface(*ty).map_err(model_error))
                .collect::<Result<Vec<_>, _>>()?;
            let pair = (
                target.identity().canonical_name(),
                canonical_trait,
                arguments,
            );
            if !seen.insert(pair) {
                diagnostics.push(MetaDiagnosticEntry::new(
                    MetaDiagnosticCode::InvalidDeriveRequest,
                    format!(
                        "trait `{trait_identity}` is derived more than once for `{target_name}`"
                    ),
                    Some(hir_request.span()),
                ));
            }
            if let Some(symbol) = trait_symbol {
                context.add_trait(trait_identity);
                if let Some(registration) = source_providers.get(symbol) {
                    let identity = registration.identity();
                    provider_locks.insert(identity.clone(), registration.locked.clone());
                    if !request_limits.contains_key(&identity) {
                        // A provider may serve several concrete instances of the
                        // same trait in one request; its artifact is immutable.
                        if !registered_sources.contains(&identity) {
                            registry
                                .insert(&identity, registration.provider.clone())
                                .map_err(model_error)?;
                            registered_sources.insert(identity.clone());
                        }
                        let locked = &registration.locked.limits;
                        request_limits.insert(
                            identity.clone(),
                            MetaLimits::new(
                                locked.steps.min(limits.steps()),
                                locked.memory_bytes.min(limits.memory_bytes()),
                                locked.output_bytes.min(limits.output_bytes()),
                            )
                            .map_err(model_error)?,
                        );
                    }
                    context.add_provider(DeriveProvider::new(
                        trait_identity
                            .split_once('[')
                            .map_or(trait_identity.as_str(), |(base, _)| base),
                        identity,
                        Vec::<String>::new(),
                    ));
                } else if symbol.identity().package() == packages.standard()
                    && symbol.identity().module().as_str() == "serialization"
                    && let Some((_, provider)) =
                        serialization_provider(&format!("serialization.{}", symbol.name()))
                {
                    context.add_provider(DeriveProvider::new(
                        trait_identity
                            .split_once('[')
                            .map_or(trait_identity.as_str(), |(base, _)| base),
                        provider,
                        used_generic_parameters(&meta_declaration, &generic_names),
                    ));
                }
            } else if let Some((_, provider)) = standard_provider {
                context.add_trait(trait_identity);
                context.add_provider(DeriveProvider::new(
                    trait_identity
                        .split_once('[')
                        .map_or(trait_identity.as_str(), |(base, _)| base),
                    provider,
                    used_generic_parameters(&meta_declaration, &generic_names),
                ));
            }
        }
        for locked in &standard.derive_providers {
            provider_locks.insert(locked.entry.clone(), locked.clone());
            request_limits.insert(
                locked.entry.clone(),
                MetaLimits::new(
                    locked.limits.steps.min(limits.steps()),
                    locked.limits.memory_bytes.min(limits.memory_bytes()),
                    locked.limits.output_bytes.min(limits.output_bytes()),
                )
                .map_err(model_error)?,
            );
        }
        let request = DeriveRequest::from_hir(module_name, hir_request);
        let plan = match validate_derive_requests(&[request], &context) {
            Ok(plan) => plan,
            Err(error) => {
                diagnostics.extend(error.diagnostics().iter().map(semantic_entry));
                continue;
            }
        };
        if !diagnostics.is_empty() {
            continue;
        }

        let source = sources
            .get(declaration.span().file())
            .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?;
        let package = packages
            .package_for_source(source.source_id())
            .ok_or_else(|| DeriveFrontendError::Invariant("derive target is unowned".into()))?;
        let mut seeds = vec![target.id()];
        let mut builtin_roots = Vec::new();
        let mut roots = BTreeSet::from([
            MetaRoot::new(package.id().as_str(), module_name).map_err(model_error)?
        ]);
        for index in 0..hir_request.traits().len() {
            if let Some(symbol) = requested_trait_symbol(parsed, resolved, hir, hir_request, index)
            {
                seeds.push(symbol.id());
                roots.insert(
                    MetaRoot::new(
                        symbol.identity().package().as_str(),
                        symbol.identity().module().as_str(),
                    )
                    .map_err(model_error)?,
                );
            } else if let Some((trait_name, _)) = standard_trait_provider(
                packages,
                resolved,
                hir_request.span().file(),
                &hir_request.traits()[index],
            ) {
                builtin_roots.push(
                    trait_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(trait_name)
                        .to_owned(),
                );
            }
        }
        let snapshot = crate::meta_snapshot::SnapshotInput {
            sources,
            parsed,
            resolved,
            hir,
            environment,
            packages,
            owner: module.package(),
        }
        .build(
            roots.into_iter().collect(),
            &seeds,
            &builtin_roots,
            &hir_request
                .bound_references()
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
            Some(target.id()),
        )?;
        let execution = match crate::meta_derive::execute_derive_plan_with_limits(
            &plan,
            snapshot.clone(),
            limits,
            &registry,
            &request_limits,
        ) {
            Ok(execution) => execution,
            Err(crate::meta_derive::DeriveExecutionError::ProviderVm {
                source: crate::meta_vm::MetaVmError::ProviderDiagnostics(messages),
                ..
            }) => {
                for message in messages {
                    let mut diagnostic = MetaDiagnosticEntry::new(
                        MetaDiagnosticCode::DeriveExpansionFailed,
                        message.message,
                        Some(hir_request.span()),
                    );
                    if let Some(origin) = message.origin {
                        diagnostic = diagnostic.with_related(
                            "provider input",
                            provider_origin(sources, origin, hir_request.span())?,
                        );
                    }
                    diagnostics.push(diagnostic);
                }
                continue;
            }
            Err(error) => {
                diagnostics.push(derive_execution_entry(
                    &error,
                    Some(hir_request.span()),
                    [(format!("derive target `{target_name}`"), declaration.span())],
                ));
                continue;
            }
        };
        let imports = source_imports(sources, parsed, declaration.span().file())?;
        for diagnostic in execution.response().diagnostics() {
            let origin = diagnostic
                .origin
                .map(|origin| provider_origin(sources, origin, hir_request.span()))
                .transpose()?;
            let severity = match diagnostic.severity {
                crate::meta::MetaDiagnosticSeverity::Note => crate::diagnostics::Severity::Note,
                crate::meta::MetaDiagnosticSeverity::Warning => {
                    crate::diagnostics::Severity::Warning
                }
                crate::meta::MetaDiagnosticSeverity::Error => crate::diagnostics::Severity::Error,
            };
            messages.push(
                MetaDiagnosticEntry::new(
                    MetaDiagnosticCode::DeriveExpansionFailed,
                    &diagnostic.message,
                    origin.or(Some(hir_request.span())),
                )
                .with_severity(severity),
            );
        }
        let publication = provenance::Publication {
            environment,
            sources,
            source,
            owner: package.id(),
            target: &target.identity().canonical_name(),
            request_span: hir_request.span(),
            snapshot: &snapshot,
            imports: &imports,
        };
        for ((trait_request, output), baseline_header) in plan[0]
            .traits()
            .iter()
            .zip(execution.response().outputs())
            .zip(execution.baseline_headers())
        {
            let (source, result, descriptor) = publication.accept(
                output,
                trait_request,
                baseline_header,
                &plan[0].written_bounds(),
                &provider_locks[trait_request.provider()],
                request_limits[trait_request.provider()],
            )?;
            generated.push(source);
            accepted.push(result);
            descriptors.push(descriptor);
        }
    }

    if diagnostics.is_empty() {
        Ok(DeriveExpansion {
            sources: generated,
            diagnostics: messages,
            accepted,
            descriptors,
        })
    } else {
        Err(DeriveFrontendError::Diagnostics(diagnostics))
    }
}

fn model_error(error: impl std::fmt::Display) -> DeriveFrontendError {
    DeriveFrontendError::Invariant(error.to_string())
}

pub(crate) fn provider_origin(
    sources: &SourceDatabase,
    origin: crate::meta::MetaSpan,
    request: Span,
) -> Result<Span, DeriveFrontendError> {
    let invalid = || {
        DeriveFrontendError::Diagnostics(vec![MetaDiagnosticEntry::new(
            MetaDiagnosticCode::InvalidGeneratedSource,
            "provider origin is not an admitted UTF-8 source range",
            Some(request),
        )])
    };
    let file = FileId::from_index(origin.file() as usize).map_err(|_| invalid())?;
    let text = sources
        .get(file)
        .map_err(|_| invalid())?
        .text()
        .map_err(|_| invalid())?;
    if !text.is_char_boundary(origin.start() as usize)
        || !text.is_char_boundary(origin.end() as usize)
    {
        return Err(invalid());
    }
    sources
        .span(
            file,
            TextRange::new(origin.start(), origin.end()).map_err(|_| invalid())?,
        )
        .map_err(|_| invalid())
}

fn requested_trait_symbol<'a>(
    parsed: &[(FileId, Parsed)],
    resolved: &'a ResolvedProgram,
    hir: &HirProgram,
    request: &crate::hir::HirDeriveRequest,
    index: usize,
) -> Option<&'a crate::resolve::Symbol> {
    let root = parsed
        .iter()
        .find(|(file, _)| *file == request.span().file())?
        .1
        .cst()
        .root_node();
    let node = root.child_nodes().find(|node| {
        node.kind() == SyntaxKind::DeriveDecl && node.range() == request.span().range()
    })?;
    let traits = node
        .child_nodes()
        .find(|node| node.kind() == SyntaxKind::DeriveTraitList)?;
    let path = traits
        .child_nodes()
        .filter(|node| node.kind() == SyntaxKind::TypePath)
        .nth(index)?;
    for token in path.descendant_tokens() {
        if token.kind() == TokenKind::LBracket {
            break;
        }
        let Some(reference) = resolved.reference(request.span().file(), token.range()) else {
            continue;
        };
        if let crate::resolve::ResolvedEntity::Name(crate::resolve::ResolvedName::Symbol(id)) =
            reference.entity()
            && hir.declaration(*id).is_some_and(|declaration| {
                matches!(declaration.kind(), HirTypeDeclarationKind::Trait(_))
            })
        {
            return resolved.symbol(*id);
        }
    }
    None
}

fn serialization_provider(identity: &str) -> Option<(&'static str, &'static str)> {
    let base = identity.split_once('[').map_or(identity, |(base, _)| base);
    Some(match base {
        ENCODE_TRAIT => (ENCODE_TRAIT, ENCODE_PROVIDER),
        DECODE_TRAIT => (DECODE_TRAIT, DECODE_PROVIDER),
        _ => return None,
    })
}

fn standard_trait_provider(
    packages: &PackageGraph,
    resolved: &ResolvedProgram,
    file: FileId,
    identity: &str,
) -> Option<(&'static str, &'static str)> {
    let base = identity.split_once('[').map_or(identity, |(base, _)| base);
    let (alias, name) = base.rsplit_once('.')?;
    let alias = crate::package::Name::new(alias).ok()?;
    let import = resolved.file(file)?.imports().get(&alias)?;
    if import.module().package() != packages.standard()
        || import.module().path().as_str() != "serialization"
    {
        return None;
    }
    serialization_provider(&format!("serialization.{name}"))
}

fn derive_target_kind(declaration: &HirTypeDeclaration) -> DeriveTargetKind {
    match declaration.kind() {
        HirTypeDeclarationKind::Alias { .. } => DeriveTargetKind::Alias,
        HirTypeDeclarationKind::Trait(_) => DeriveTargetKind::Other,
        HirTypeDeclarationKind::Nominal(definition) => match definition.shape() {
            HirNominalShape::Record { .. } => DeriveTargetKind::Record,
            HirNominalShape::Enum { .. } => DeriveTargetKind::Enum,
            HirNominalShape::Newtype { .. } => DeriveTargetKind::Newtype,
        },
    }
}

pub(crate) fn declaration_syntax(
    parsed: &[(FileId, Parsed)],
    span: Span,
) -> Option<SyntaxNodeRef<'_>> {
    let root = parsed
        .iter()
        .find_map(|(file, parsed)| (*file == span.file()).then(|| parsed.cst().root_node()))?;
    root.child_nodes().find(|node| {
        matches!(
            node.kind(),
            SyntaxKind::TypeDecl
                | SyntaxKind::EnumDecl
                | SyntaxKind::AliasDecl
                | SyntaxKind::TraitDecl
        ) && range_contains(node.range(), span.range())
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_meta_declaration(
    packages: &PackageGraph,
    owner: &crate::package::PackageId,
    sources: &SourceDatabase,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    symbol: &crate::resolve::Symbol,
    declaration: &HirTypeDeclaration,
    syntax: SyntaxNodeRef<'_>,
    module: &str,
) -> Result<MetaDeclaration, DeriveFrontendError> {
    let file = declaration.span().file();
    let authored_generics = meta_generics(sources, file, syntax)?;
    let generics = crate::meta_binders::parameters(hir, resolved, declaration.parameters())
        .map_err(model_error)?
        .into_iter()
        .zip(authored_generics)
        .map(|(parameter, authored)| parameter.with_source_bounds(authored.bounds().to_vec()))
        .collect::<Vec<_>>();
    let bounds = crate::meta_binders::bounds(&generics).map_err(model_error)?;
    let names = crate::meta_binders::names(hir, resolved, declaration.parameters(), None)
        .map_err(model_error)?;
    let type_ref = |ty, source: String| {
        crate::meta_type::MetaTypeRef::resolved(hir, resolved, packages, owner, ty, source, &names)
            .map_err(model_error)
    };
    let mut trait_methods = match declaration.kind() {
        HirTypeDeclarationKind::Trait(definition) => {
            definition.methods().iter().collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    // HIR indexes members by identity. Model ordinals follow authored order.
    trait_methods
        .sort_by_key(|method| resolved.member(method.member()).map(|member| member.span()));
    let kind = match declaration.kind() {
        HirTypeDeclarationKind::Alias { target } => MetaDeclarationKind::Alias(type_ref(
            *target,
            hir.interner().canonical(*target).map_err(model_error)?,
        )?),
        HirTypeDeclarationKind::Trait(definition) => MetaDeclarationKind::trait_definition(
            trait_methods
                .into_iter()
                .enumerate()
                .map(|(ordinal, method)| {
                    let member = resolved
                        .member(method.member())
                        .ok_or_else(|| model_error("trait operation member is missing"))?;
                    let signature = hir
                        .callable(crate::hir::HirCallableId::Member(method.member()))
                        .ok_or_else(|| model_error("trait operation signature is missing"))?;
                    let names = crate::meta_binders::names(
                        hir,
                        resolved,
                        signature.generics(),
                        Some(definition.self_type()),
                    )
                    .map_err(model_error)?;
                    let local_parameters = signature
                        .generics()
                        .iter()
                        .filter(|parameter| {
                            parameter.position() as usize > declaration.parameters().len()
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let local_parameters =
                        crate::meta_binders::parameters(hir, resolved, &local_parameters)
                            .map_err(model_error)?;
                    crate::meta::MetaOperation::new(
                        member.name().as_str(),
                        crate::meta_type::MetaTypeRef::resolved(
                            hir,
                            resolved,
                            packages,
                            owner,
                            signature.function_type(),
                            hir.interner()
                                .canonical_interface(signature.function_type())
                                .map_err(model_error)?,
                            &names,
                        )
                        .map_err(model_error)?,
                        meta_visibility(member.visibility()),
                        ordinal as u32,
                        member.span(),
                        member.docs(),
                    )
                    .map(|operation| {
                        operation.with_contract(
                            local_parameters,
                            method.has_default(),
                            method.requires_self_send(),
                        )
                    })
                    .map_err(model_error)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        HirTypeDeclarationKind::Nominal(definition) => match definition.shape() {
            HirNominalShape::Newtype { underlying } => {
                let ty = direct_child(syntax, SyntaxKind::TypeExpr)
                    .ok_or_else(|| DeriveFrontendError::Invariant("newtype has no type".into()))?;
                MetaDeclarationKind::newtype(type_ref(
                    *underlying,
                    compact_syntax(sources, declaration.span().file(), ty)?,
                )?)
            }
            HirNominalShape::Record { fields } => MetaDeclarationKind::record(
                fields
                    .iter()
                    .enumerate()
                    .map(|(ordinal, field)| {
                        meta_field(sources, resolved, file, syntax, field, ordinal, &type_ref)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            HirNominalShape::Enum { variants } => MetaDeclarationKind::enumeration(
                variants
                    .iter()
                    .enumerate()
                    .map(|(ordinal, variant)| {
                        let member = resolved.member(variant.member()).ok_or_else(|| {
                            DeriveFrontendError::Invariant("enum variant member is missing".into())
                        })?;
                        let node = containing_node(syntax, SyntaxKind::EnumVariant, member.span())
                            .ok_or_else(|| {
                                DeriveFrontendError::Invariant(
                                    "enum variant syntax is missing".into(),
                                )
                            })?;
                        let payload = match variant.payload() {
                            HirVariantPayload::Unit => MetaVariantPayload::unit(),
                            HirVariantPayload::Tuple(types) => {
                                let tuple = direct_child(node, SyntaxKind::TuplePayload)
                                    .ok_or_else(|| {
                                        DeriveFrontendError::Invariant(
                                            "tuple variant payload is missing".into(),
                                        )
                                    })?;
                                MetaVariantPayload::tuple(
                                    tuple
                                        .child_nodes()
                                        .filter(|child| child.kind() == SyntaxKind::TypeExpr)
                                        .zip(types)
                                        .map(|(node, ty)| {
                                            type_ref(
                                                *ty,
                                                compact_syntax(
                                                    sources,
                                                    member.span().file(),
                                                    node,
                                                )?,
                                            )
                                        })
                                        .collect::<Result<Vec<_>, _>>()?,
                                )
                            }
                            HirVariantPayload::Record(fields) => MetaVariantPayload::record(
                                fields
                                    .iter()
                                    .enumerate()
                                    .map(|(ordinal, field)| {
                                        meta_field(
                                            sources,
                                            resolved,
                                            member.span().file(),
                                            node,
                                            field,
                                            ordinal,
                                            &type_ref,
                                        )
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                            ),
                        };
                        MetaVariant::new(
                            member.name().as_str(),
                            payload,
                            u32::try_from(ordinal).map_err(|_| {
                                DeriveFrontendError::Invariant("variant ordinal overflow".into())
                            })?,
                            member.span(),
                            member.docs(),
                        )
                        .map_err(model_error)?
                        .with_attributes(meta_attributes(sources, member.span().file(), node)?)
                        .map_err(model_error)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        },
    };
    MetaDeclaration::new(
        symbol.name().as_str(),
        module,
        meta_visibility(symbol.visibility()),
        generics,
        bounds,
        declaration.span(),
        None::<String>,
        kind,
    )
    .map_err(model_error)
}

#[allow(clippy::too_many_arguments)]
fn meta_field(
    sources: &SourceDatabase,
    resolved: &ResolvedProgram,
    file: FileId,
    root: SyntaxNodeRef<'_>,
    field: &crate::hir::HirField,
    ordinal: usize,
    type_ref: &impl Fn(
        crate::types::TypeId,
        String,
    ) -> Result<crate::meta_type::MetaTypeRef, DeriveFrontendError>,
) -> Result<MetaField, DeriveFrontendError> {
    let member = resolved
        .member(field.member())
        .ok_or_else(|| DeriveFrontendError::Invariant("record field member is missing".into()))?;
    let node = containing_node(root, SyntaxKind::RecordField, member.span())
        .ok_or_else(|| DeriveFrontendError::Invariant("record field syntax is missing".into()))?;
    let ty = direct_child(node, SyntaxKind::TypeExpr)
        .ok_or_else(|| DeriveFrontendError::Invariant("record field type is missing".into()))?;
    MetaField::new(
        member.name().as_str(),
        type_ref(field.ty(), compact_syntax(sources, file, ty)?)?,
        meta_visibility(member.visibility()),
        u32::try_from(ordinal)
            .map_err(|_| DeriveFrontendError::Invariant("field ordinal overflow".into()))?,
        member.span(),
        member.docs(),
    )
    .map_err(model_error)?
    .with_attributes(meta_attributes(sources, file, node)?)
    .map_err(model_error)
}

fn meta_generics(
    sources: &SourceDatabase,
    file: FileId,
    declaration: SyntaxNodeRef<'_>,
) -> Result<Vec<MetaGenericParameter>, DeriveFrontendError> {
    let Some(parameters) = direct_child(declaration, SyntaxKind::GenericParams) else {
        return Ok(Vec::new());
    };
    parameters
        .child_nodes()
        .filter(|node| node.kind() == SyntaxKind::GenericParam)
        .map(|parameter| {
            let name = parameter
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
                .and_then(|token| token.token().normalized_identifier())
                .ok_or_else(|| {
                    DeriveFrontendError::Invariant("generic parameter name is missing".into())
                })?;
            let bounds = direct_child(parameter, SyntaxKind::GenericBound)
                .map(|bound| {
                    bound
                        .child_nodes()
                        .filter(|node| node.kind() == SyntaxKind::TypePath)
                        .map(|node| compact_syntax(sources, file, node))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            MetaGenericParameter::new(name, bounds).map_err(model_error)
        })
        .collect()
}

fn meta_attributes(
    sources: &SourceDatabase,
    file: FileId,
    owner: SyntaxNodeRef<'_>,
) -> Result<Vec<MetaAttribute>, DeriveFrontendError> {
    owner
        .child_nodes()
        .filter(|node| node.kind() == SyntaxKind::Attribute)
        .map(|attribute| {
            let identifiers = attribute
                .descendant_tokens()
                .take_while(|token| token.kind() != TokenKind::LParen)
                .filter(|token| token.kind() == TokenKind::Identifier)
                .filter_map(|token| token.token().normalized_identifier())
                .collect::<Vec<_>>();
            let name = identifiers.join(".");
            let bytes = sources
                .get(file)
                .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?
                .bytes();
            let text = std::str::from_utf8(
                &bytes[attribute.range().start() as usize..attribute.range().end() as usize],
            )
            .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?;
            let argument = text
                .find('(')
                .zip(text.rfind(')'))
                .and_then(|(start, end)| (start < end).then(|| text[start + 1..end].trim()))
                .filter(|value| !value.is_empty())
                .map(|value| {
                    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_owned())
                });
            MetaAttribute::new(name, argument).map_err(model_error)
        })
        .collect()
}

fn compact_syntax(
    sources: &SourceDatabase,
    file: FileId,
    node: SyntaxNodeRef<'_>,
) -> Result<String, DeriveFrontendError> {
    let bytes = sources
        .get(file)
        .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?
        .bytes();
    let mut output = String::new();
    let mut previous_word = false;
    for token in node.descendant_tokens() {
        if token.is_synthetic()
            || token.kind().is_trivia()
            || matches!(token.kind(), TokenKind::Nl | TokenKind::Eof)
        {
            continue;
        }
        let range = token.range();
        let text = token
            .token()
            .normalized_identifier()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                String::from_utf8_lossy(&bytes[range.start() as usize..range.end() as usize])
                    .into_owned()
            });
        let word = text
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_alphanumeric());
        if previous_word && word {
            output.push(' ');
        }
        output.push_str(&text);
        previous_word = word;
    }
    if output.is_empty() {
        Err(DeriveFrontendError::Invariant(
            "meta type spelling is empty".into(),
        ))
    } else {
        Ok(output)
    }
}

fn direct_child(node: SyntaxNodeRef<'_>, kind: SyntaxKind) -> Option<SyntaxNodeRef<'_>> {
    node.child_nodes().find(|child| child.kind() == kind)
}

fn containing_node(
    root: SyntaxNodeRef<'_>,
    kind: SyntaxKind,
    span: Span,
) -> Option<SyntaxNodeRef<'_>> {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.kind() == kind && range_contains(node.range(), span.range()) {
            return Some(node);
        }
        pending.extend(node.child_nodes());
    }
    None
}

fn range_contains(outer: crate::source::TextRange, inner: crate::source::TextRange) -> bool {
    outer.start() <= inner.start() && inner.end() <= outer.end()
}

fn source_imports(
    sources: &SourceDatabase,
    parsed: &[(FileId, Parsed)],
    file: FileId,
) -> Result<Vec<Vec<u8>>, DeriveFrontendError> {
    let parsed = parsed
        .iter()
        .find_map(|(candidate, parsed)| (*candidate == file).then_some(parsed))
        .ok_or_else(|| DeriveFrontendError::Invariant("derive source was not parsed".into()))?;
    let bytes = sources
        .get(file)
        .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?
        .bytes();
    let mut seen = BTreeSet::new();
    let mut imports = Vec::new();
    for node in parsed
        .cst()
        .root_node()
        .child_nodes()
        .filter(|node| node.kind() == SyntaxKind::ImportDecl)
    {
        let import = bytes[node.range().start() as usize..node.range().end() as usize].to_vec();
        if seen.insert(import.clone()) {
            imports.push(import);
        }
    }
    Ok(imports)
}

fn used_generic_parameters(declaration: &MetaDeclaration, parameters: &[String]) -> Vec<String> {
    let mut types = Vec::new();
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => {
            types.extend(fields.iter().map(|field| field.ty()));
        }
        MetaDeclarationKind::Enum(variants) => {
            for variant in variants {
                match variant.payload() {
                    MetaVariantPayload::Unit => {}
                    MetaVariantPayload::Tuple(items) => {
                        types.extend(items.iter().map(crate::meta_type::MetaTypeRef::source));
                    }
                    MetaVariantPayload::Record(fields) => {
                        types.extend(fields.iter().map(|field| field.ty()));
                    }
                }
            }
        }
        MetaDeclarationKind::Newtype(ty) => types.push(ty.source()),
        MetaDeclarationKind::Trait(_)
        | MetaDeclarationKind::Alias(_)
        | MetaDeclarationKind::Function(_)
        | MetaDeclarationKind::Constant(_) => {}
    }
    parameters
        .iter()
        .filter(|parameter| {
            types.iter().any(|ty| {
                ty.split(|character: char| !(character == '_' || character.is_alphanumeric()))
                    .any(|word| word == parameter.as_str())
            })
        })
        .cloned()
        .collect()
}

fn meta_visibility(visibility: Visibility) -> MetaVisibility {
    match visibility {
        Visibility::Public => MetaVisibility::Public,
        Visibility::Private => MetaVisibility::Private,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE_PROVIDER: &str = r#"
import std.meta
pub fn expand(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    let model = request.snapshot()
    assert(model.declarations.length() == 2)
    assert(model.declarations[0].identity == "User")
    assert(model.declarations[1].identity == "ValueOf")
    match model.declarations[1].kind {
        meta.DeclarationKind.Trait(operations) => assert(operations.length() == 1)
        _ => assert(false)
    }
    let source = "impl {request.traitIdentity()} for {request.target()} {{\nfn value(self):Int{{self.secret}}\n}}\n"
    let target = match model.declarations[0].origin {
        meta.Origin.Source(span) => span
        meta.Origin.Builtin(_) => panic("expected source origin")
    }
    ok(meta.DeriveResponse { source: source,
        diagnostics: [meta.Diagnostic { severity: meta.DiagnosticSeverity.Note,
            message: "generated value accessor", origin: some(request.span()) }],
        mappings: [meta.SourceMap { generatedStart: 0u32, generatedEnd: 4u32,
            originFile: target.file, originStart: target.start, originEnd: target.end }] })
}
"#;

    fn source_derive_request(
        provider: &str,
        source: &str,
        matching_package: bool,
        steps: u64,
    ) -> crate::driver::CompilationRequest {
        use crate::driver::*;
        use crate::meta_provider::{
            SourceDeriveProviders, SourceDeriveRegistration, SourceMetaProvider,
        };
        use crate::package::{Edition, PackageGraph};
        use crate::source::*;
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:app").unwrap(),
                ModulePath::new("app").unwrap(),
                LogicalPath::new("src/app.to").unwrap(),
                source.as_bytes(),
            ))
            .unwrap();
        let packages = PackageGraph::loose(&sources, root).unwrap();
        let trait_package = if matching_package {
            packages.root().as_str().into()
        } else {
            "workspace:other@1".into()
        };
        let compiled = SourceMetaProvider::compile(
            crate::meta_test_support::source_request(provider),
            "provider.expand",
            crate::meta_vm::MetaEntryKind::Derive,
        )
        .unwrap();
        let mut providers = SourceDeriveProviders::default();
        providers
            .insert(SourceDeriveRegistration {
                locked: crate::toolchain::LockedDeriveProvider {
                    origin: "manifest".into(),
                    trait_package,
                    trait_module: "app".into(),
                    trait_name: "ValueOf".into(),
                    provider_package: "workspace:provider@1".into(),
                    entry: "provider.expand".into(),
                    provider_hash: compiled.hash().unwrap(),
                    meta_model: crate::meta::META_MODEL.into(),
                    limits: crate::toolchain::Limits {
                        steps,
                        memory_bytes: 1_048_576,
                        output_bytes: 8_192,
                    },
                },
                provider: std::sync::Arc::new(compiled),
            })
            .unwrap();
        CompilationRequest::new(
            Operation::Run,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            Default::default(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        )
        .unwrap()
        .with_source_derive_providers(providers)
    }

    const DERIVING_SOURCE: &str = "trait ValueOf {\n    fn value(self): Int\n}\ntype User = { secret: Int }\ntype Unrelated = { hidden: String }\nderive ValueOf for User\nfn extract[T: ValueOf + Discard](item: T): Int { item.value() }\nfn main() {\n    assert(extract(User { secret: 42 }) == 42)\n}\n";

    #[test]
    fn meta_source_derive_accepts_only_necessary_added_bounds() {
        let source = "trait ValueOf {\nfn value(self): Int\n}\ntrait ReadValue {\nfn read(self): Int\n}\nimpl ReadValue for Int {\nfn read(self): Int { self }\n}\ntype User[T: Discard] = { secret: T }\nderive[T] ValueOf for User[T]\nfn extract[T: ValueOf + Discard](item: T): Int { item.value() }\nfn main() { assert(extract(User[Int] { secret: 42 }) == 42) }\n";
        let implementation = "impl[T: Discard + ReadValue] ValueOf for User[T] {\nfn value(self): Int { self.secret.read() }\n}\n";
        let manual = source.replace("derive[T] ValueOf for User[T]", implementation);
        let manual_output = crate::driver::execute(source_derive_request(
            SOURCE_PROVIDER,
            &manual,
            true,
            100_000,
        ))
        .unwrap();
        assert_eq!(
            manual_output.exit_code(),
            0,
            "{}",
            manual_output.diagnostics().human()
        );
        for (extra, expected) in [("", 0), (" + Copy", 1)] {
            let generated = implementation.replace(
                "Discard + ReadValue",
                &format!("Discard + ReadValue{extra}"),
            );
            let literal =
                serde_json::to_string(&generated.replace('{', "{{").replace('}', "}}")).unwrap();
            let provider = format!(
                "import std.meta\npub fn expand(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {{\nreturn ok(meta.DeriveResponse {{ source: {literal}, diagnostics: [], mappings: [] }})\n}}\n"
            );
            let output =
                crate::driver::execute(source_derive_request(&provider, source, true, 100_000))
                    .unwrap();
            assert_eq!(
                output.exit_code(),
                expected,
                "extra={extra}: {}",
                output.diagnostics().human()
            );
            if expected != 0 {
                assert!(output.diagnostics().json_lines().unwrap().contains("E2105"));
                assert!(
                    output
                        .diagnostics()
                        .human()
                        .contains("redundant bound `T: Copy`")
                );
                assert!(output.artifact().is_none());
            }
        }
    }

    #[test]
    fn meta_source_derive_preserves_written_bounds_and_their_model_closure() {
        let source = "trait ValueOf {\nfn value(self): Int\n}\ntype User[T: Discard] = { secret: T }\nderive[T: Copy] ValueOf for User[T]\nfn extract[T: ValueOf + Discard](item: T): Int { item.value() }\nfn main() { assert(extract(User[Int] { secret: 42 }) == 42) }\n";
        let provider = r#"
import std.meta
pub fn expand(request: meta.DeriveRequest): meta.DeriveResponse ! meta.Error {
    assert(request.bounds() == ["T: Copy"])
    let model = request.snapshot()
    var found = false
    for declaration in model.declarations {
        if declaration.module == "prelude" and declaration.identity == "Copy" {
            found = true
        }
    }
    assert(found)
    ok(meta.DeriveResponse { source: "impl[T: Copy + Discard] ValueOf for User[T] {{\nfn value(self): Int {{ 42 }}\n}}\n", diagnostics: [], mappings: [] })
}
"#;
        for (provider, expected) in [
            (provider.to_owned(), 0),
            (provider.replace("Copy + Discard", "Discard"), 1),
        ] {
            let output =
                crate::driver::execute(source_derive_request(&provider, source, true, 100_000))
                    .unwrap();
            assert_eq!(
                output.exit_code(),
                expected,
                "{}",
                output.diagnostics().human()
            );
            if expected != 0 {
                assert!(output.diagnostics().json_lines().unwrap().contains("E2105"));
                assert!(output.artifact().is_none());
            }
        }
    }

    #[test]
    fn meta_source_frontend_executes_ordinary_derive_and_retains_notes() {
        let output = crate::driver::execute(source_derive_request(
            SOURCE_PROVIDER,
            DERIVING_SOURCE,
            true,
            100_000,
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            crate::driver::CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0);
        let messages = output.diagnostics().json_lines().unwrap();
        assert!(messages.contains("generated value accessor"), "{messages}");
        assert!(messages.contains("\"severity\":\"note\""), "{messages}");
        assert!(messages.contains("src/app.to"));
    }

    #[test]
    fn meta_source_frontend_rejects_wrong_package_and_enforces_locked_budget() {
        for (matching_package, steps, expected) in [(false, 100_000, "E2102"), (true, 1, "E2107")] {
            let output = crate::driver::execute(source_derive_request(
                SOURCE_PROVIDER,
                DERIVING_SOURCE,
                matching_package,
                steps,
            ))
            .unwrap();
            assert_eq!(output.status(), crate::driver::CompilationStatus::Rejected);
            assert!(
                output
                    .diagnostics()
                    .json_lines()
                    .unwrap()
                    .contains(expected),
                "{}",
                output.diagnostics().human()
            );
        }
    }

    #[test]
    fn meta_derive_compiler_diagnostics_use_composed_source_maps() {
        let generated = "impl ValueOf for User {\nfn value(self):Int{true}\n}\n";
        let provider = SOURCE_PROVIDER.replace("self.secret", "true").replace(
            "generatedEnd: 4u32",
            &format!("generatedEnd: {}u32", generated.len()),
        );
        let output = crate::driver::execute(source_derive_request(
            &provider,
            DERIVING_SOURCE,
            true,
            100_000,
        ))
        .unwrap();
        assert_eq!(output.status(), crate::driver::CompilationStatus::Rejected);
        assert!(output.interface().is_none());
        assert!(output.artifact().is_none());
        let diagnostics = output
            .diagnostics()
            .json_lines()
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["severity"] == "error")
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{}", output.diagnostics().human());
        assert_eq!(errors[0]["source_id"], "root:app");
        assert_eq!(errors[0]["file"], "src/app.to");
        assert_eq!(
            errors[0]["range"]["start"]["byte"],
            DERIVING_SOURCE.find("User =").unwrap()
        );
    }

    #[test]
    fn source_serialization_providers_are_only_the_canonical_traits() {
        assert_eq!(
            serialization_provider("serialization.Encode[Json]"),
            Some((ENCODE_TRAIT, ENCODE_PROVIDER))
        );
        assert_eq!(
            serialization_provider("serialization.Decode[MessagePack]"),
            Some((DECODE_TRAIT, DECODE_PROVIDER))
        );
        assert_eq!(serialization_provider("serialization.Serialize"), None);
        assert_eq!(serialization_provider("serialization.Deserialize"), None);
    }
}
