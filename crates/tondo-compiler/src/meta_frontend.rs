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
    MetaLimits, MetaModule, MetaRoot, MetaSnapshot, MetaVariant, MetaVariantPayload,
    MetaVisibility, validate_derive_requests,
};
use crate::meta_derive::{DeriveProviderRegistry, execute_derive_plan};
use crate::meta_diagnostics::{MetaDiagnosticEntry, derive_execution_entry, semantic_entry};
use crate::package::PackageGraph;
use crate::resolve::{ResolvedProgram, Visibility};
use crate::serialization_derive::{
    DECODE_PROVIDER, DECODE_TRAIT, ENCODE_PROVIDER, ENCODE_TRAIT, register_serialization_providers,
};
use crate::source::{FileId, ModulePath, SourceDatabase, SourceId, Span};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, TokenKind};

#[derive(Debug)]
pub(crate) struct GeneratedDeriveSource {
    pub source_id: SourceId,
    pub module: ModulePath,
    pub path: String,
    pub bytes: Vec<u8>,
    pub diagnostic_origin: Span,
}

#[derive(Debug)]
pub(crate) enum DeriveFrontendError {
    Diagnostics(Vec<MetaDiagnosticEntry>),
    Invariant(String),
}

pub(crate) fn expand_derives(
    packages: &PackageGraph,
    sources: &SourceDatabase,
    parsed: &[(FileId, Parsed)],
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    limits: MetaLimits,
) -> Result<Vec<GeneratedDeriveSource>, DeriveFrontendError> {
    if hir.derive_requests().is_empty() {
        return Ok(Vec::new());
    }

    let mut registry = DeriveProviderRegistry::default();
    register_serialization_providers(&mut registry)
        .map_err(|error| DeriveFrontendError::Invariant(error.to_string()))?;
    let mut seen = BTreeSet::new();
    let mut generated = Vec::new();
    let mut diagnostics = Vec::new();

    for (request_index, hir_request) in hir.derive_requests().iter().enumerate() {
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
        for trait_identity in hir_request.traits() {
            let pair = (
                module_name.to_owned(),
                target_name.to_owned(),
                trait_identity.clone(),
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
            if let Some((base, provider)) = serialization_provider(trait_identity) {
                context.add_trait(trait_identity);
                context.add_provider(DeriveProvider::new(
                    base,
                    provider,
                    used_generic_parameters(&meta_declaration, &generic_names),
                ));
            }
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
        let snapshot = MetaSnapshot::new(
            [MetaRoot::new(package.id().as_str(), module_name).map_err(model_error)?],
            [MetaModule::new(module_name, None::<String>).map_err(model_error)?],
            [meta_declaration],
        )
        .map_err(model_error)?;
        let execution = match execute_derive_plan(&plan, snapshot, limits, &registry) {
            Ok(execution) => execution,
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
        for (trait_index, output) in execution.response().outputs().iter().enumerate() {
            let mut bytes = Vec::new();
            for import in &imports {
                bytes.extend_from_slice(import);
                if !import.ends_with(b"\n") {
                    bytes.push(b'\n');
                }
            }
            bytes.extend_from_slice(output.bytes());
            generated.push(GeneratedDeriveSource {
                source_id: source.source_id().clone(),
                module: source.module().clone(),
                path: format!("generated/derive/{request_index:08}-{trait_index:04}.to"),
                bytes,
                diagnostic_origin: declaration.span(),
            });
        }
    }

    if diagnostics.is_empty() {
        Ok(generated)
    } else {
        Err(DeriveFrontendError::Diagnostics(diagnostics))
    }
}

fn model_error(error: impl std::fmt::Display) -> DeriveFrontendError {
    DeriveFrontendError::Invariant(error.to_string())
}

fn serialization_provider(identity: &str) -> Option<(&'static str, &'static str)> {
    let base = identity.split_once('[').map_or(identity, |(base, _)| base);
    Some(match base {
        ENCODE_TRAIT => (ENCODE_TRAIT, ENCODE_PROVIDER),
        DECODE_TRAIT => (DECODE_TRAIT, DECODE_PROVIDER),
        _ => return None,
    })
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

fn declaration_syntax(parsed: &[(FileId, Parsed)], span: Span) -> Option<SyntaxNodeRef<'_>> {
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

fn build_meta_declaration(
    sources: &SourceDatabase,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    symbol: &crate::resolve::Symbol,
    declaration: &HirTypeDeclaration,
    syntax: SyntaxNodeRef<'_>,
    module: &str,
) -> Result<MetaDeclaration, DeriveFrontendError> {
    let file = declaration.span().file();
    let generics = meta_generics(sources, file, syntax)?;
    let kind = match declaration.kind() {
        HirTypeDeclarationKind::Alias { target } => {
            MetaDeclarationKind::newtype(hir.interner().canonical(*target).map_err(model_error)?)
        }
        HirTypeDeclarationKind::Trait(_) => MetaDeclarationKind::trait_definition([]),
        HirTypeDeclarationKind::Nominal(definition) => match definition.shape() {
            HirNominalShape::Newtype { .. } => {
                let ty = direct_child(syntax, SyntaxKind::TypeExpr)
                    .ok_or_else(|| DeriveFrontendError::Invariant("newtype has no type".into()))?;
                MetaDeclarationKind::newtype(compact_syntax(
                    sources,
                    declaration.span().file(),
                    ty,
                )?)
            }
            HirNominalShape::Record { fields } => MetaDeclarationKind::record(
                fields
                    .iter()
                    .enumerate()
                    .map(|(ordinal, field)| {
                        meta_field(sources, resolved, file, syntax, field, ordinal)
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
                            HirVariantPayload::Tuple(_) => {
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
                                        .map(|ty| compact_syntax(sources, member.span().file(), ty))
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
                            member.span().into(),
                            None::<String>,
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
        [],
        declaration.span().into(),
        None::<String>,
        kind,
    )
    .map_err(model_error)
}

fn meta_field(
    sources: &SourceDatabase,
    resolved: &ResolvedProgram,
    file: FileId,
    root: SyntaxNodeRef<'_>,
    field: &crate::hir::HirField,
    ordinal: usize,
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
        compact_syntax(sources, file, ty)?,
        meta_visibility(member.visibility()),
        u32::try_from(ordinal)
            .map_err(|_| DeriveFrontendError::Invariant("field ordinal overflow".into()))?,
        member.span().into(),
        None::<String>,
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
                        types.extend(items.iter().map(String::as_str));
                    }
                    MetaVariantPayload::Record(fields) => {
                        types.extend(fields.iter().map(|field| field.ty()));
                    }
                }
            }
        }
        MetaDeclarationKind::Newtype(ty) => types.push(ty),
        MetaDeclarationKind::Trait(_) => {}
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
