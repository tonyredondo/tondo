//! Select authored signatures before ordinary name/type resolution. Projection
//! preserves byte positions and is used only for the pre-generation model; the
//! final pipeline always checks the original complete source and generated batch.

use std::collections::{BTreeMap, BTreeSet};

use super::{GenerationError, invariant, meta_failure};
use crate::driver::{CompilationRequest, DriverError};
use crate::meta::MetaDiagnosticCode;
use crate::package::{ImportResolutionError, ModuleId, Name, Namespace};
use crate::source::{FileId, SourceDatabase, SourceInput, TextRange};
use crate::syntax::{
    LexMode, ParseLimits, ParseMode, Parsed, SyntaxKind, SyntaxNodeRef, TokenKind, lex, parse,
};

struct Declaration<'a> {
    file: FileId,
    module: ModuleId,
    node: SyntaxNodeRef<'a>,
    namespace: Namespace,
    name: Option<String>,
    public: bool,
}

struct Import {
    range: TextRange,
    target: Option<ModuleId>,
}

pub(super) fn project(
    request: &CompilationRequest,
    parsed: &[(FileId, Parsed)],
    roots: &BTreeSet<ModuleId>,
    outputs: &BTreeSet<(String, String)>,
) -> Result<Vec<(FileId, Parsed)>, GenerationError> {
    let packages = request.packages();
    let mut declarations = Vec::new();
    let mut imports = BTreeMap::new();
    for (file, syntax) in parsed {
        let module = packages
            .module_for_file(request.sources(), *file)
            .map_err(DriverError::from)?;
        for node in syntax.cst().root_node().child_nodes() {
            if node.kind() == SyntaxKind::ImportDecl {
                if let Some(path) = node
                    .child_nodes()
                    .find(|node| node.kind() == SyntaxKind::ModulePath)
                {
                    let names = identifiers(path);
                    let alias = node
                        .child_tokens()
                        .find(|token| token.kind() == TokenKind::Identifier)
                        .and_then(|token| token.token().normalized_identifier())
                        .map(str::to_owned)
                        .or_else(|| names.last().cloned());
                    let segments = names
                        .iter()
                        .map(Name::new_path_component)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(invariant)?;
                    let target = match packages.resolve_import(module.package(), &segments) {
                        Ok(target) | Err(ImportResolutionError::UnknownModule(target)) => {
                            Some(target)
                        }
                        Err(_) => None,
                    };
                    if let Some(alias) = alias {
                        imports.insert(
                            (*file, alias),
                            Import {
                                range: node.range(),
                                target,
                            },
                        );
                    }
                }
                continue;
            }
            let namespace = if matches!(
                node.kind(),
                SyntaxKind::FunctionDecl | SyntaxKind::ConstDecl
            ) {
                Namespace::Value
            } else {
                Namespace::Type
            };
            let name = match node.kind() {
                SyntaxKind::TypeDecl
                | SyntaxKind::AliasDecl
                | SyntaxKind::EnumDecl
                | SyntaxKind::TraitDecl
                | SyntaxKind::ConstDecl => identifiers(node).into_iter().next(),
                SyntaxKind::FunctionDecl => node
                    .child_nodes()
                    .find(|node| node.kind() == SyntaxKind::FunctionHead)
                    .map(|head| {
                        head.child_tokens()
                            .filter(|token| token.kind() == TokenKind::Identifier)
                            .filter_map(|token| token.token().normalized_identifier())
                            .collect::<Vec<_>>()
                            .join(".")
                    }),
                SyntaxKind::ImplDecl | SyntaxKind::DeriveDecl => None,
                _ => continue,
            };
            declarations.push(Declaration {
                file: *file,
                module: module.clone(),
                node,
                namespace,
                name,
                public: node
                    .child_nodes()
                    .any(|node| node.kind() == SyntaxKind::Visibility),
            });
        }
    }
    let mut by_name: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (index, declaration) in declarations.iter().enumerate() {
        if let Some(name) = &declaration.name {
            by_name
                .entry((
                    declaration.module.clone(),
                    declaration.namespace,
                    name.clone(),
                ))
                .or_default()
                .push(index);
        }
    }
    let mut pending = declarations
        .iter()
        .enumerate()
        .filter_map(|(index, declaration)| {
            ((roots.contains(&declaration.module) && declaration.public)
                || declaration.node.kind() == SyntaxKind::DeriveDecl
                || declaration.module.package() == packages.standard())
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut selected = BTreeSet::new();
    let mut used_imports = BTreeSet::new();
    let mut derive_targets = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if !selected.insert(index) {
            continue;
        }
        let declaration = &declarations[index];
        if outputs.contains(&(
            declaration.module.package().as_str().into(),
            declaration.module.path().as_str().into(),
        )) {
            return Err(meta_failure(
                request,
                MetaDiagnosticCode::GenerationDependencyCycle,
                format!(
                    "meta signature closure depends on current-round output `{}`",
                    declaration.module
                ),
            )?);
        }
        let binders = signature_nodes(declaration.node)
            .into_iter()
            .filter(|node| node.kind() == SyntaxKind::GenericParam)
            .filter_map(|node| identifiers(node).into_iter().next())
            .collect::<BTreeSet<_>>();
        for node in signature_nodes(declaration.node) {
            let names = match node.kind() {
                SyntaxKind::TypePath => path_names(node),
                SyntaxKind::FunctionHead => {
                    let names = node
                        .child_tokens()
                        .filter(|token| token.kind() == TokenKind::Identifier)
                        .filter_map(|token| {
                            token.token().normalized_identifier().map(str::to_owned)
                        })
                        .collect::<Vec<_>>();
                    if names.len() != 2 {
                        continue;
                    }
                    vec![names[0].clone()]
                }
                _ => continue,
            };
            let Some(first) = names.first() else {
                continue;
            };
            if binders.contains(first) || first == "Self" {
                continue;
            }
            let (module, name) =
                if let Some(import) = imports.get(&(declaration.file, first.clone())) {
                    used_imports.insert((declaration.file, first.clone()));
                    let Some(module) = &import.target else {
                        continue;
                    };
                    if outputs.contains(&(
                        module.package().as_str().into(),
                        module.path().as_str().into(),
                    )) {
                        return Err(meta_failure(
                            request,
                            MetaDiagnosticCode::GenerationDependencyCycle,
                            format!(
                                "meta signature closure depends on current-round output `{module}`"
                            ),
                        )?);
                    }
                    let Some(name) = names.get(1) else {
                        continue;
                    };
                    (module, name)
                } else {
                    (&declaration.module, first)
                };
            if let Some(indices) = by_name.get(&(module.clone(), Namespace::Type, name.clone())) {
                pending.extend(indices);
                if declaration.node.kind() == SyntaxKind::DeriveDecl
                    && declaration.node.child_nodes().any(|target| {
                        target.kind() == SyntaxKind::DeriveTarget
                            && target
                                .child_nodes()
                                .any(|path| path.range() == node.range())
                    })
                {
                    // Only the exact derive target receives the private view.
                    // Revisit a previously public root when this grant arrives.
                    for index in indices {
                        if derive_targets.insert(*index) {
                            selected.remove(index);
                        }
                    }
                }
            }
        }
        // Visible implementation headers are needed when their target is part
        // of the closure. Their method bodies are still excluded below.
        if declaration.namespace == Namespace::Type
            && let Some(name) = &declaration.name
        {
            for (candidate, implementation) in declarations.iter().enumerate() {
                if implementation.node.kind() == SyntaxKind::FunctionDecl
                    && implementation.module == declaration.module
                    && (implementation.public || derive_targets.contains(&index))
                    && implementation.name.as_deref().is_some_and(|method| {
                        method
                            .split_once('.')
                            .is_some_and(|(owner, _)| owner == name)
                    })
                {
                    pending.push(candidate);
                }
                if implementation.node.kind() != SyntaxKind::ImplDecl {
                    continue;
                }
                let target = implementation
                    .node
                    .child_nodes()
                    .filter(|node| node.kind() == SyntaxKind::TypePath)
                    .last();
                if let Some(target) = target {
                    let names = path_names(target);
                    let local =
                        implementation.module == declaration.module && names.first() == Some(name);
                    let imported = names.get(1) == Some(name)
                        && names.first().is_some_and(|alias| {
                            imports
                                .get(&(implementation.file, alias.clone()))
                                .and_then(|import| import.target.as_ref())
                                == Some(&declaration.module)
                        });
                    if local || imported {
                        pending.push(candidate);
                    }
                }
            }
        }
    }
    let mut ranges: BTreeMap<FileId, Vec<TextRange>> = BTreeMap::new();
    let mut initializers = BTreeMap::new();
    for index in selected {
        let declaration = &declarations[index];
        ranges
            .entry(declaration.file)
            .or_default()
            .push(declaration.node.range());
        for node in signature_nodes(declaration.node) {
            if node.kind() == SyntaxKind::ConstDecl
                && let Some(expression) = node
                    .child_nodes()
                    .find_map(crate::syntax::ast::Expression::cast)
            {
                initializers
                    .entry(declaration.file)
                    .or_insert_with(Vec::new)
                    .push(expression.syntax().range());
            }
        }
    }
    for key in used_imports {
        ranges.entry(key.0).or_default().push(imports[&key].range);
    }
    let mut shadow = SourceDatabase::new();
    for (file, original) in request.sources().iter() {
        let mut bytes = original
            .bytes()
            .iter()
            .map(|byte| {
                if matches!(byte, b'\r' | b'\n') {
                    *byte
                } else {
                    b' '
                }
            })
            .collect::<Vec<_>>();
        if let Some(kept) = ranges.get(&file) {
            for range in kept {
                let span = range.start() as usize..range.end() as usize;
                bytes[span.clone()].copy_from_slice(&original.bytes()[span]);
            }
            if let Some((_, syntax)) = parsed.iter().find(|(id, _)| *id == file) {
                let mut pending = vec![syntax.cst().root_node()];
                while let Some(node) = pending.pop() {
                    if node.kind() == SyntaxKind::Block {
                        // No executable source is used in a semantic signature.
                        // CST ranges also include trivia before the opening
                        // brace, so use the actual delimiter token positions.
                        let open = node
                            .child_tokens()
                            .find(|token| token.kind() == TokenKind::LBrace)
                            .ok_or_else(|| invariant("authored block has no opening brace"))?;
                        let close = node
                            .child_tokens()
                            .filter(|token| token.kind() == TokenKind::RBrace)
                            .last()
                            .ok_or_else(|| invariant("authored block has no closing brace"))?;
                        erase(
                            &mut bytes,
                            TextRange::new(open.range().end(), close.range().start())
                                .map_err(DriverError::from)?,
                        );
                    } else {
                        pending.extend(node.child_nodes());
                    }
                }
            }
            if let Some(expressions) = initializers.get(&file) {
                for range in expressions {
                    erase(&mut bytes, *range);
                    if let Some(byte) = bytes[range.start() as usize..range.end() as usize]
                        .iter_mut()
                        .find(|byte| !matches!(byte, b'\r' | b'\n'))
                    {
                        *byte = b'0';
                    }
                }
            }
        }
        let added = shadow
            .add(SourceInput::new(
                original.source_id().clone(),
                original.module().clone(),
                original.path().clone(),
                original.origin(),
                bytes,
            ))
            .map_err(DriverError::from)?;
        if added != file {
            return Err(invariant("signature projection changed file identity"));
        }
    }
    let mut projected = Vec::new();
    for (file, original) in parsed {
        if !ranges.contains_key(file) {
            continue;
        }
        let script = original.cst().root_node().kind() == SyntaxKind::Script;
        let lexed = lex(
            &shadow,
            *file,
            if script {
                LexMode::Script
            } else {
                LexMode::Module
            },
        )
        .map_err(DriverError::from)?;
        let syntax = parse(
            &shadow,
            *file,
            lexed,
            if script {
                ParseMode::Script
            } else {
                ParseMode::Module
            },
            ParseLimits::default(),
        )
        .map_err(DriverError::from)?;
        projected.push((*file, syntax));
    }
    Ok(projected)
}

fn identifiers(node: SyntaxNodeRef<'_>) -> Vec<String> {
    node.descendant_tokens()
        .filter(|token| token.kind() == TokenKind::Identifier)
        .filter_map(|token| token.token().normalized_identifier().map(str::to_owned))
        .collect()
}

pub(super) fn authored_spans(
    sources: &SourceDatabase,
    original: &[(FileId, Parsed)],
    projected: &[(FileId, Parsed)],
) -> Result<BTreeMap<crate::source::Span, crate::source::Span>, GenerationError> {
    let mut spans = BTreeMap::new();
    let anchor = |node: SyntaxNodeRef<'_>| {
        node.descendant_tokens()
            .find(|token| {
                !token.kind().is_trivia() && !matches!(token.kind(), TokenKind::Nl | TokenKind::Eof)
            })
            .map(|token| token.range())
    };
    for (file, syntax) in projected {
        let authored = original
            .iter()
            .find(|(id, _)| id == file)
            .ok_or_else(|| invariant("signature projection lost its source"))?;
        let declarations = authored
            .1
            .cst()
            .root_node()
            .child_nodes()
            .filter_map(|node| anchor(node).map(|anchor| (anchor, node)))
            .collect::<BTreeMap<_, _>>();
        for node in syntax.cst().root_node().child_nodes() {
            let Some(anchor) = anchor(node) else { continue };
            let original = declarations
                .get(&anchor)
                .filter(|original| original.kind() == node.kind())
                .ok_or_else(|| invariant("signature projection changed a declaration anchor"))?;
            spans.insert(
                sources
                    .span(*file, node.range())
                    .map_err(DriverError::from)?,
                sources
                    .span(*file, original.range())
                    .map_err(DriverError::from)?,
            );
        }
    }
    Ok(spans)
}

fn path_names(node: SyntaxNodeRef<'_>) -> Vec<String> {
    node.child_tokens()
        .take_while(|token| token.kind() != TokenKind::LBracket)
        .filter(|token| token.kind() == TokenKind::Identifier)
        .filter_map(|token| token.token().normalized_identifier().map(str::to_owned))
        .collect()
}

fn signature_nodes(root: SyntaxNodeRef<'_>) -> Vec<SyntaxNodeRef<'_>> {
    let mut pending = vec![root];
    let mut nodes = Vec::new();
    while let Some(node) = pending.pop() {
        if node.kind() == SyntaxKind::Block {
            continue;
        }
        if node.kind() != SyntaxKind::ConstDecl {
            pending.extend(node.child_nodes());
        } else {
            pending.extend(
                node.child_nodes()
                    .filter(|node| node.kind() == SyntaxKind::TypeExpr),
            );
        }
        nodes.push(node);
    }
    nodes
}

fn erase(bytes: &mut [u8], range: TextRange) {
    let start = range.start() as usize;
    let end = range.end() as usize;
    for byte in &mut bytes[start..end] {
        if !matches!(byte, b'\r' | b'\n') {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_signature_projection_restores_authored_derive_and_impl_ranges() {
        let request = crate::meta_test_support::source_request(
            "pub trait ValueOf { fn value(self): Int\n}\n\
             pub trait Marker {}\n\
             pub type User = { value: Int }\n\
             derive ValueOf for User\n\
             fn omittedFirst(): Int { 1 }\n\
             impl Marker for User {}\n\
             fn omittedSecond(): Int { 2 }\n\
             pub fn answer(): Int { 42 }\n",
        );
        let sources = request.sources();
        let file = request.root();
        let parsed = parse(
            sources,
            file,
            lex(sources, file, LexMode::Module).unwrap(),
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(parsed.diagnostics().is_empty());
        let roots = BTreeSet::from([request.packages().module_for_file(sources, file).unwrap()]);
        let original = vec![(file, parsed)];
        let projected = project(&request, &original, &roots, &BTreeSet::new())
            .unwrap_or_else(|_| panic!("projection failed"));
        let spans = authored_spans(sources, &original, &projected)
            .unwrap_or_else(|_| panic!("source binding failed"));
        let selected = || projected.iter().map(|(file, parsed)| (*file, parsed));
        let (resolved, diagnostics) =
            crate::resolve::resolve(request.packages(), sources, selected(), 100)
                .unwrap()
                .into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (mut hir, diagnostics) = crate::hir::lower_types(
            request.packages(),
            sources,
            selected(),
            &resolved,
            crate::hir::TypeLoweringLimits {
                max_type_nodes: request.limits().max_type_nodes,
                max_trait_obligations: request.limits().max_trait_obligations,
                max_diagnostics: 100,
            },
        )
        .unwrap()
        .into_parts();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let authored = |kind| {
            let range = original[0]
                .1
                .cst()
                .root_node()
                .child_nodes()
                .find(|node| node.kind() == kind)
                .unwrap()
                .range();
            sources.span(file, range).unwrap()
        };
        assert_ne!(
            hir.derive_requests()[0].span(),
            authored(SyntaxKind::DeriveDecl)
        );
        assert_ne!(
            hir.implementations().next().unwrap().span(),
            authored(SyntaxKind::ImplDecl)
        );
        hir.restore_meta_declaration_spans(&spans);
        assert_eq!(
            hir.derive_requests()[0].span(),
            authored(SyntaxKind::DeriveDecl)
        );
        assert_eq!(
            hir.implementations().next().unwrap().span(),
            authored(SyntaxKind::ImplDecl)
        );
    }
}
